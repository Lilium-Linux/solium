//! Qt, served between frames (#164).
//!
//! [`super::tick`] drains Qt's events only on a frame that is drawn, so on an
//! idle desktop a QML `Timer` never fired, nothing Qt waits for on a pipe or a
//! socket arrived, and a hosted clock stood still. This puts Qt's own poll set
//! in the compositor's event loop, as one descriptor registered once: it turns
//! readable when Qt's next timer is due, or when any descriptor Qt's dispatcher
//! waits on is ready ([`super::poll_set`]). Then Qt is served on the
//! compositor's clock ([`super::drain`]), and a scene that changed asks for one
//! frame through the backend's ordinary `redraw` flag.
//!
//! That clock is the one frames advance, and every `Timer` rides it while any
//! animation runs. So while an animation runs that no frame is coming to
//! advance, the descriptor also turns readable a frame of Qt's own driver
//! later, and the wake advances the clock as that frame would have.
//!
//! `tests::a_ready_descriptor_reaches_its_scene_with_no_frame_drawn`,
//! `tests::a_timer_beside_an_undrawn_animation_fires_with_no_frame_drawn`; and
//! nothing due, nothing armed: `tests::an_idle_host_does_not_wake_repeatedly`.

use std::cell::Cell;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use smithay::reexports::{
    calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic},
    rustix::{
        self,
        event::epoll,
        time::{Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec},
    },
};

use super::PollFd;
use crate::power::Step;

/// The deadlines Qt reports are whole milliseconds, so deadlines closer than
/// one are the same deadline.
const SLACK: Duration = Duration::from_millis(1);

/// One frame of Qt's own animation driver, which is how often the clock is
/// stepped for an animation no frame is drawing: `QDefaultAnimationDriver`
/// runs a precise timer at `DEFAULT_TIMER_INTERVAL`, 16 ms (qtbase v6.11.2,
/// src/corelib/animation/qabstractanimation.cpp:128 and :903-907).
/// `tests::a_timer_beside_an_undrawn_animation_fires_with_no_frame_drawn`.
const FRAME: Duration = Duration::from_millis(16);

/// `G_IO_IN`, `G_IO_PRI` and `G_IO_OUT` (glib/giochannel.h), which on Linux
/// are `poll`'s own bits.
/// `tests::a_ready_descriptor_reaches_its_scene_with_no_frame_drawn`.
const G_IO_IN: u16 = 1;
const G_IO_PRI: u16 = 2;
const G_IO_OUT: u16 = 4;

/// Whether the backend's next frame will advance the clock without anything
/// else happening: one is wanted, and some monitor will draw a picture for it,
/// or is still flipping (`None`) and will ask again when the flip lands. A
/// monitor that is blanking, going dark or resting draws no picture, so no
/// QML is ticked for it. `tests::a_frame_is_coming_only_if_a_monitor_will_draw_it`.
pub(crate) fn frame_coming(wanted: bool, monitors: impl IntoIterator<Item = Option<Step>>) -> bool {
    wanted
        && monitors
            .into_iter()
            .any(|step| matches!(step, None | Some(Step::Draw | Step::Wake)))
}

/// Qt's poll set, as the one descriptor the event loop watches for it.
///
/// Two epolls, and neither is calloop's. `outer` is registered with the loop
/// once and holds Qt's timer and `glib`, which holds exactly the descriptors
/// GLib's last query named, with the events it named. A calloop source per
/// descriptor would put GLib's descriptors in calloop's own epoll, and an
/// epoll entry cannot be deleted once its descriptor is closed: it lives for
/// as long as the file does, and a file is open for as long as anything holds
/// it -- a dup, a child that inherited it. Readable, level-triggered and
/// nobody's, it would spin the whole loop. In a set of our own, rebuilt
/// whenever the query changes rather than edited, it goes with the set:
/// `tests::a_descriptor_qt_stops_watching_is_let_go_while_it_stays_open`.
#[derive(Debug)]
struct PollSet {
    outer: OwnedFd,
    /// GLib's descriptors, and what they were registered from: the query, and
    /// the file behind each descriptor.
    /// `tests::a_descriptor_qt_stops_watching_is_let_go_while_it_stays_open`.
    glib: Option<(OwnedFd, Watched)>,
}

/// What a set of GLib's descriptors was registered from: each entry of the
/// query, and the file behind its descriptor when it has one.
/// `tests::a_descriptor_number_reused_for_another_file_is_watched`.
type Watched = Vec<(PollFd, Option<File>)>;

/// Which file a descriptor is: its device and inode.
///
/// Compared as well as the query, because a query can come back the same while
/// the files under it changed. A child process restarted from its own exit
/// handler closes its pipes and opens new ones within one drain, and the new
/// ones take the lowest free numbers -- the old ones. The old entries went with
/// their files, so an unchanged query would leave the new pipes watched by
/// nobody: `tests::a_descriptor_number_reused_for_another_file_is_watched`.
type File = (u64, u64);

impl PollSet {
    fn new(timer: &OwnedFd) -> Result<Self> {
        let outer = epoll::create(epoll::CreateFlags::CLOEXEC)
            .map_err(|err| anyhow!("creating Qt's poll set: {err}"))?;
        epoll::add(
            &outer,
            timer,
            epoll::EventData::new_u64(0),
            epoll::EventFlags::IN,
        )
        .map_err(|err| anyhow!("putting Qt's timer in its poll set: {err}"))?;
        Ok(Self { outer, glib: None })
    }

    /// Watch exactly `wanted`.
    /// `tests::a_descriptor_qt_stops_watching_is_let_go_while_it_stays_open`.
    #[expect(unsafe_code, reason = "borrowing descriptors GLib owns")]
    fn watch(&mut self, wanted: &[PollFd]) {
        let wanted: Watched = wanted
            .iter()
            .map(|poll| {
                let file = (poll.fd >= 0).then(|| {
                    // SAFETY: as below; the borrow ends with the call.
                    let fd = unsafe { BorrowedFd::borrow_raw(poll.fd) };
                    rustix::fs::fstat(fd)
                        .ok()
                        .map(|stat| (stat.st_dev, stat.st_ino))
                });
                (*poll, file.flatten())
            })
            .collect();
        if self.glib.as_ref().is_some_and(|(_, had)| *had == wanted) {
            return;
        }
        let glib = match epoll::create(epoll::CreateFlags::CLOEXEC) {
            Ok(glib) => glib,
            Err(err) => {
                tracing::warn!(?err, "could not create a set for Qt's descriptors");
                return;
            }
        };
        for (poll, _) in wanted.iter().filter(|(poll, _)| poll.fd >= 0) {
            // SAFETY: GLib's query named this descriptor a moment ago on this
            // thread, and nothing of GLib's has run since, so it is open; the
            // borrow ends with the call. `arm` is the only caller that passes
            // GLib's: `tests::a_ready_descriptor_reaches_its_scene_with_no_frame_drawn`.
            let fd = unsafe { BorrowedFd::borrow_raw(poll.fd) };
            if let Err(err) = epoll::add(
                &glib,
                fd,
                epoll::EventData::new_u64(0),
                interest(poll.events),
            ) {
                tracing::warn!(
                    fd = poll.fd,
                    ?err,
                    "could not watch a descriptor Qt waits on"
                );
            }
        }
        if let Err(err) = epoll::add(
            &self.outer,
            &glib,
            epoll::EventData::new_u64(1),
            epoll::EventFlags::IN,
        ) {
            tracing::warn!(?err, "could not put Qt's descriptors in its poll set");
            return;
        }
        if let Some((old, _)) = self.glib.take() {
            let _ = epoll::delete(&self.outer, &old);
        }
        self.glib = Some((glib, wanted));
    }
}

/// What epoll is asked to report for a descriptor GLib polls for `events`.
/// `tests::a_ready_descriptor_reaches_its_scene_with_no_frame_drawn`.
fn interest(events: u16) -> epoll::EventFlags {
    let mut interest = epoll::EventFlags::empty();
    for (bit, flag) in [
        (G_IO_IN, epoll::EventFlags::IN),
        (G_IO_PRI, epoll::EventFlags::PRI),
        (G_IO_OUT, epoll::EventFlags::OUT),
    ] {
        if events & bit != 0 {
            interest |= flag;
        }
    }
    interest
}

/// Qt's poll set in the event loop, and the timer it wakes on.
#[derive(Debug)]
pub(crate) struct Wake {
    /// A second descriptor for the timer in the poll set, kept to arm it.
    /// `tests::a_timer_fires_while_no_frame_is_drawn`.
    timer: OwnedFd,
    /// Where the timer was last pointed, or `None` while it is disarmed.
    armed: Option<Instant>,
    set: PollSet,
    /// What the last query named, kept to be refilled.
    /// `tests::a_ready_descriptor_reaches_its_scene_with_no_frame_drawn`.
    fds: Vec<PollFd>,
    /// Whether a frame was coming when the set was armed, as the wake reads
    /// it. `tests::a_coming_frame_is_left_to_advance_the_clock`.
    coming: Rc<Cell<bool>>,
    /// When a wake last advanced the clock, as the wake writes it.
    /// `tests::a_timer_beside_an_undrawn_animation_fires_with_no_frame_drawn`.
    stepped: Rc<Cell<Option<Instant>>>,
}

impl Wake {
    /// Put Qt's poll set in `handle`'s loop. `now` is the compositor's clock,
    /// the one [`super::tick`] is fed; `woke` is told, on every wake, whether
    /// serving Qt turned a clean scene dirty.
    /// `tests::an_animation_a_timer_starts_between_frames_starts_at_the_timer`.
    pub(crate) fn insert<D: 'static>(
        handle: &LoopHandle<'_, D>,
        now: impl Fn(&D) -> Duration + 'static,
        mut woke: impl FnMut(&mut D, bool) + 'static,
    ) -> Result<Self> {
        let timer = rustix::time::timerfd_create(
            TimerfdClockId::Monotonic,
            TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
        )
        .map_err(|err| anyhow!("creating the timer Qt's timers wake on: {err}"))?;
        let set = PollSet::new(&timer)?;
        let polled = set
            .outer
            .try_clone()
            .map_err(|err| anyhow!("duplicating Qt's poll set: {err}"))?;
        let expired = timer
            .try_clone()
            .map_err(|err| anyhow!("duplicating the timer Qt's timers wake on: {err}"))?;
        let coming = Rc::new(Cell::new(false));
        let stepped = Rc::new(Cell::new(None));
        let (frame, step) = (Rc::clone(&coming), Rc::clone(&stepped));
        handle
            .insert_source(
                Generic::new(polled, Interest::READ, Mode::Level),
                move |_, _, data| {
                    let mut expirations = [0_u8; 8];
                    let _ = rustix::io::read(&expired, &mut expirations[..]);
                    let advance = !frame.get();
                    if advance {
                        step.set(Some(Instant::now()));
                    }
                    let elapsed = now(data);
                    let changed = super::drain(elapsed, advance);
                    woke(data, changed);
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| anyhow!("watching Qt's poll set: {err}"))?;
        Ok(Self {
            timer,
            armed: None,
            set,
            fds: Vec::new(),
            coming,
            stepped,
        })
    }

    /// Point the poll set at what Qt waits for now.
    ///
    /// Once per loop iteration, after the frame and before the loop sleeps:
    /// anything that ran in the iteration may have started or stopped a Qt
    /// timer, opened or closed a descriptor, or started an animation.
    /// `frame_coming` is [`frame_coming`]'s answer: while a frame is coming, its
    /// tick advances the clock, and a wake only delivers.
    /// `tests::an_idle_host_does_not_wake_repeatedly`.
    pub(crate) fn arm(&mut self, frame_coming: bool) {
        self.coming.set(frame_coming);
        let now = Instant::now();
        let due = super::poll_set(&mut self.fds).and_then(|due| now.checked_add(due));
        self.set.watch(&self.fds);
        // A frame's interval after the clock was last stepped here, and not
        // after `now`: a loop woken every few milliseconds by something else
        // would otherwise push the step back every time and never take it.
        // `tests::a_timer_beside_an_undrawn_animation_fires_with_no_frame_drawn`.
        let step = (!frame_coming && super::animating()).then(|| {
            self.stepped
                .get()
                .and_then(|at| at.checked_add(FRAME))
                .map_or(now, |at| at.max(now))
        });
        let wanted = match (due, step) {
            (Some(due), Some(step)) => Some(due.min(step)),
            (due, step) => due.or(step),
        };
        self.point(now, wanted);
    }

    /// Point the timer at `wanted`, or disarm it.
    /// `tests::an_idle_host_does_not_wake_repeatedly`.
    fn point(&mut self, now: Instant, wanted: Option<Instant>) {
        let armed = self.armed.filter(|at| *at > now);
        let unchanged = match (armed, wanted) {
            (Some(armed), Some(wanted)) => armed.max(wanted) - armed.min(wanted) < SLACK,
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            self.armed = armed;
            return;
        }
        // A zero value disarms a timerfd, so "due now" is a nanosecond. Qt's
        // first answer for a new Timer is "now": its start is a posted event.
        // `tests::a_timer_fires_while_no_frame_is_drawn`.
        let value = wanted.map_or(Duration::ZERO, |at| {
            at.saturating_duration_since(now)
                .max(Duration::from_nanos(1))
        });
        let spec = Itimerspec {
            it_interval: Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: Timespec {
                tv_sec: i64::try_from(value.as_secs()).unwrap_or(i64::MAX),
                tv_nsec: value.subsec_nanos().into(),
            },
        };
        match rustix::time::timerfd_settime(&self.timer, TimerfdTimerFlags::empty(), &spec) {
            Ok(_) => self.armed = wanted,
            Err(err) => tracing::warn!(?err, "could not arm the timer Qt's timers wake on"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use smithay::reexports::{calloop::EventLoop, rustix};

    use super::Wake;
    use crate::qml::{Scene, qt_test::on_the_qt_thread};

    /// What one window of the loop saw.
    #[derive(Debug, Default)]
    struct Seen {
        /// Times the loop came back from waiting.
        iterations: u32,
        /// Times Qt's poll set was what woke it.
        wakes: u32,
        /// Wakes that turned a clean scene dirty.
        changed: u32,
        redraw: bool,
    }

    /// The compositor's clock, for every test here: one origin for the whole
    /// process, so the animation clock only ever moves forward.
    fn now() -> Duration {
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        ORIGIN.get_or_init(Instant::now).elapsed()
    }

    /// The compositor's loop with nothing in it but Qt's poll set: no clients,
    /// no input, and no frame coming, so nothing but a wake moves the
    /// animation clock. `draw` stands in for the frame a backend draws when
    /// `redraw` is set.
    fn run(window: Duration, draw: impl FnMut()) -> Seen {
        run_until(window, || false, draw)
    }

    /// [`run`], ending early once `done` says so after a wake.
    fn run_until(window: Duration, done: impl FnMut() -> bool, draw: impl FnMut()) -> Seen {
        run_with(window, false, done, draw)
    }

    /// [`run_until`], telling the poll set whether a frame is `coming`.
    fn run_with(
        window: Duration,
        coming: bool,
        mut done: impl FnMut() -> bool,
        mut draw: impl FnMut(),
    ) -> Seen {
        let mut event_loop: EventLoop<Seen> = EventLoop::try_new().expect("an event loop");
        let mut wake = Wake::insert(
            &event_loop.handle(),
            |_: &Seen| now(),
            |seen: &mut Seen, changed| {
                seen.wakes += 1;
                if changed {
                    seen.changed += 1;
                    seen.redraw = true;
                }
            },
        )
        .expect("Qt's poll set");
        let mut seen = Seen::default();
        let start = Instant::now();
        loop {
            if std::mem::take(&mut seen.redraw) {
                draw();
            }
            wake.arm(coming);
            let left = window.saturating_sub(start.elapsed());
            if left.is_zero() {
                return seen;
            }
            event_loop
                .dispatch(Some(left), &mut seen)
                .expect("dispatching the loop");
            seen.iterations += 1;
            if done() {
                return seen;
            }
        }
    }

    /// Whether the loop would wake for `set` now.
    fn readable(set: &super::PollSet) -> bool {
        let mut fds = [rustix::event::PollFd::new(
            &set.outer,
            rustix::event::PollFlags::IN,
        )];
        let zero = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        rustix::event::poll(&mut fds, Some(&zero)).expect("polling the set") > 0
    }

    /// A scene built from `qml`, the way a hosted shell's is, in a fresh
    /// directory that the caller removes.
    fn build(name: &str, qml: &str) -> (PathBuf, Scene) {
        build_beside(name, &[], qml)
    }

    /// [`build`], with other files written into the scene's directory first.
    ///
    /// What an earlier test's scene left queued is delivered before this one
    /// is built. A scene dropped mid-animation leaves the unregistering of its
    /// animation timer queued, and a Timer started before that is delivered is
    /// handed the dropped animation's time as its first step:
    /// `a_timer_fires_while_no_frame_is_drawn` fired eleven times in a second
    /// right after `a_coming_frame_is_left_to_advance_the_clock`.
    fn build_beside(name: &str, files: &[(&str, &str)], qml: &str) -> (PathBuf, Scene) {
        crate::qml::start().expect("Qt starts");
        let _ = run(Duration::from_millis(50), || {});
        let directory = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        for (file, contents) in files {
            std::fs::write(directory.join(file), contents)
                .expect("writing a file beside the scene");
        }
        let path = directory.join("Scene.qml");
        std::fs::write(&path, qml).expect("writing the scene");
        crate::qml::start().expect("Qt starts");
        let scene = Scene::for_host(&path, 64, 16, None).expect("the scene builds");
        (directory, scene)
    }

    /// **A `Timer` fires while the compositor draws no frames.**
    ///
    /// The defect in #164: Qt's events were drained only inside a drawn frame,
    /// so on an idle desktop a hosted shell's `Timer` never fired at all. Here
    /// nothing draws and nothing ticks for a second, and a 100 ms Timer must
    /// still have fired most of the ten times it is due. The Timer changes
    /// nothing that is drawn, and the scene has never been drawn, so no wake
    /// may ask for a frame.
    #[test]
    fn a_timer_fires_while_no_frame_is_drawn() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-timer",
                r"
                import QtQuick

                Item {
                    id: root
                    property int fired: 0
                    Timer {
                        interval: 100
                        running: true
                        repeat: true
                        onTriggered: root.fired += 1
                    }
                }
                ",
            );

            let seen = run(Duration::from_secs(1), || {});
            let fired = scene.get_int("fired");
            assert!(
                (7..=10).contains(&fired),
                "a 100 ms Timer fired {fired} times in a second with no frame drawn: {seen:?}"
            );
            // Dirty since it was built and never drawn, so no wake changed it:
            // a scene already waiting for its frame is not asked for again.
            assert_eq!(
                seen.changed, 0,
                "wakes re-announced a scene that was already waiting: {seen:?}"
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A clock repaints once a second, with no other damage.**
    ///
    /// The shape every bar clock has: a `Text` a one-second `Timer` rewrites.
    /// Over three and a half seconds of nothing else, the scene must turn dirty
    /// and ask for a frame three times, and each of those frames must actually
    /// render something new. More would be a clock paid for between its ticks;
    /// fewer is the frozen clock.
    #[test]
    fn a_clock_scene_repaints_once_a_second_with_no_other_damage() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-clock",
                r#"
                import QtQuick

                Item {
                    Text {
                        id: clock
                        text: Qt.formatTime(new Date(), "hh:mm:ss")
                    }
                    Timer {
                        interval: 1000
                        running: true
                        repeat: true
                        onTriggered: clock.text = Qt.formatTime(new Date(), "hh:mm:ss")
                    }
                }
                "#,
            );
            // The scene is dirty from birth; the frame that first shows it.
            assert!(
                scene.render().expect("the first frame").changed,
                "a new scene had nothing to draw"
            );

            let mut repaints = 0_u32;
            let seen = run(Duration::from_millis(3500), || {
                if scene.render().expect("a frame").changed {
                    repaints += 1;
                }
            });
            assert_eq!(
                seen.changed, 3,
                "the clock asked for a frame {} times in 3.5 s: {seen:?}",
                seen.changed
            );
            assert_eq!(
                repaints, 3,
                "the clock was redrawn {repaints} times in 3.5 s: {seen:?}"
            );
            // One wake per tick, and one for the Timer's start.
            assert!(
                seen.wakes <= seen.changed + 1,
                "the loop woke between the clock's ticks: {seen:?}"
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **An idle host does not wake the loop repeatedly.**
    ///
    /// Waking for Qt must cost nothing when Qt has nothing to do. A one-shot
    /// Timer is served, and then, with nothing scheduled, a second and a half
    /// of the loop is one wait: no Qt wake at all. A busy loop, or a poll at
    /// the frame rate, would come back hundreds or ninety times. One wake is
    /// allowed for a timer an earlier test left in this process.
    #[test]
    fn an_idle_host_does_not_wake_repeatedly() {
        on_the_qt_thread(|| {
            let (directory, scene) = build(
                "solium-qml-test-wake-idle",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool fired: false
                    Timer {
                        interval: 200
                        running: true
                        onTriggered: root.fired = true
                    }
                }
                ",
            );

            let busy = run(Duration::from_secs(1), || {});
            assert!(
                scene.get_bool("fired"),
                "the one-shot Timer never fired: {busy:?}"
            );

            let idle = run(Duration::from_millis(1500), || {});
            assert!(
                idle.wakes <= 1 && idle.iterations <= 2,
                "with nothing scheduled the loop still woke: {idle:?} (the busy second: {busy:?})"
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A paused animation does not wake an idle loop.**
    ///
    /// `paused` leaves an animation's `running` true while it takes the
    /// animation off the clock. A hidden spinner on the common
    /// `paused: !visible` idiom is exactly that, and counted as animating it
    /// kept an otherwise idle desktop stepping the clock a frame at a time
    /// for nothing. Here an endless animation is paused, and after it settles
    /// a second of the loop must be one wait, with the scene no longer
    /// animating.
    #[test]
    fn a_paused_animation_does_not_wake_an_idle_loop() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-paused",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool hidden: false
                    property real turn: 0
                    NumberAnimation on turn {
                        from: 0
                        to: 360
                        duration: 1000
                        loops: Animation.Infinite
                        paused: root.hidden
                    }
                }
                ",
            );

            scene.set_bool("hidden", true);
            let settling = run(Duration::from_millis(200), || {});
            let idle = run(Duration::from_secs(1), || {});
            let animating = scene.animation_in_flight();

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert!(
                !animating,
                "a paused animation still counts as animating its scene"
            );
            assert!(
                idle.wakes <= 1 && idle.iterations <= 2,
                "a paused animation kept the loop waking: {idle:?} (settling: {settling:?})"
            );
        });
    }

    /// **An animation a `Timer` starts between frames starts where the Timer
    /// fired**, not at the last frame drawn.
    ///
    /// No frame is drawn for more than a second. Then a Timer fires between
    /// frames and starts a 260 ms animation, and the first frame after it is
    /// drawn 16 ms later. That frame must show the animation a step in, not at
    /// its end: an animation clock left at the last drawn frame hands the new
    /// animation the whole idle gap as its first step, and a fade on a clock
    /// that changes once a second would be over on its first frame, every
    /// second.
    #[test]
    fn an_animation_a_timer_starts_between_frames_starts_at_the_timer() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-started",
                r"
                import QtQuick

                Item {
                    id: root
                    property real value: 0
                    readonly property int permille: Math.round(root.value * 1000)
                    property bool started: false
                    NumberAnimation {
                        id: fade
                        target: root
                        property: 'value'
                        from: 0
                        to: 1
                        duration: 260
                    }
                    Timer {
                        interval: 1100
                        running: true
                        onTriggered: {
                            root.started = true
                            fade.start()
                        }
                    }
                }
                ",
            );
            // The last frame drawn before the desktop went still.
            crate::qml::tick(now());

            let seen = run_until(Duration::from_secs(3), || scene.get_bool("started"), || {});
            assert!(
                scene.get_bool("started"),
                "the Timer never fired between frames: {seen:?}"
            );
            std::thread::sleep(Duration::from_millis(16));
            // The first frame after it.
            crate::qml::tick(now());
            let permille = scene.get_int("permille");
            assert!(
                permille < 200,
                "the first frame after the Timer showed a 260 ms animation {permille}/1000 of \
                 the way through: it was handed the idle gap as its first step ({seen:?})"
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **An animation started between two steps of the clock takes its first
    /// step from the next one.**
    ///
    /// Scenes start animations outside any drain or frame -- an input handler,
    /// a Wayland request, a scene being built -- so when a drain comes before
    /// the next frame, a Timer due or a descriptor ready, the animation is
    /// already running by then. It has taken no step, though, and the last
    /// step can be long ago. Here nothing steps the clock for 300 ms, a
    /// property write starts a 260 ms animation, and the frame drawn 16 ms
    /// after the next drain must show it a step in, not over. With no drain
    /// between: `an_animation_started_after_an_idle_gap_takes_a_frame_first`.
    #[test]
    fn an_animation_started_between_steps_starts_at_the_next() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-between",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool go: false
                    property real value: 0
                    readonly property int permille: Math.round(root.value * 1000)
                    NumberAnimation {
                        target: root
                        property: 'value'
                        from: 0
                        to: 1
                        duration: 260
                        running: root.go
                    }
                }
                ",
            );
            // The last step, on a desktop with nothing animating.
            crate::qml::tick(now());
            std::thread::sleep(Duration::from_millis(300));
            scene.set_bool("go", true);
            crate::qml::drain(now(), true);
            std::thread::sleep(Duration::from_millis(16));
            crate::qml::tick(now());
            let permille = scene.get_int("permille");
            assert!(
                permille < 200,
                "the first frame showed a 260 ms animation, started between two steps, \
                 {permille}/1000 of the way through: it was handed the time since the last step"
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **An animation started after an idle gap takes a frame's step first**,
    /// with no drain between.
    ///
    /// Input reaches a scene synchronously: a click writes a property, the
    /// property starts an animation, and the frame the click asks for is the
    /// next thing to touch Qt -- no Timer is due and nothing drains first.
    /// Here nothing has stepped the clock for 300 ms when the write starts a
    /// 260 ms animation, and each of the two frames after it, 20 ms apart,
    /// must show it a step or two in. A first step handed the whole gap ends
    /// the animation before it is ever seen.
    #[test]
    fn an_animation_started_after_an_idle_gap_takes_a_frame_first() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-gap",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool go: false
                    property real value: 0
                    readonly property int permille: Math.round(root.value * 1000)
                    NumberAnimation {
                        target: root
                        property: 'value'
                        from: 0
                        to: 1
                        duration: 260
                        running: root.go
                    }
                }
                ",
            );
            // The last step, on a desktop with nothing animating.
            crate::qml::tick(now());
            std::thread::sleep(Duration::from_millis(300));
            scene.set_bool("go", true);
            let mut shown = Vec::new();
            for _ in 0..2 {
                // More than 16 ms, so that each frame's tick drains Qt's queue.
                std::thread::sleep(Duration::from_millis(20));
                crate::qml::tick(now());
                shown.push(scene.get_int("permille"));
            }

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert!(
                shown.iter().all(|permille| *permille < 200),
                "the two frames after a write that started a 260 ms animation, 300 ms after the \
                 last step, showed it at {shown:?} permille: it was handed the gap"
            );
        });
    }

    /// **An animation started on the frame after another ended takes a
    /// frame's step first**, beside a `Timer`.
    ///
    /// A Timer keeps QML's animation timer registered and paused, so an
    /// animation started beside it starts the animation driver from inside the
    /// events a frame delivers, before that frame steps the clock. On the frame
    /// after another animation ended, the clock's origin was still where that
    /// one had started, so the new animation was handed the whole of the old
    /// one's run as its first step. Here a 300 ms animation runs to its end
    /// beside an idle Timer, a one-second animation starts on the next frame,
    /// and each of the two frames after it, 20 ms apart, must show it less
    /// than 150 of its 1000 permille in.
    #[test]
    fn an_animation_started_as_another_ends_takes_a_frame_first() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-handover",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool first: false
                    property bool second: false
                    property real lead: 0
                    property real value: 0
                    readonly property int led: Math.round(root.lead * 1000)
                    readonly property int permille: Math.round(root.value * 1000)
                    NumberAnimation {
                        target: root
                        property: 'lead'
                        from: 0
                        to: 1
                        duration: 300
                        running: root.first
                    }
                    NumberAnimation {
                        target: root
                        property: 'value'
                        from: 0
                        to: 1
                        duration: 1000
                        running: root.second
                    }
                    Timer {
                        interval: 60000
                        running: true
                    }
                }
                ",
            );
            // The Timer's start, delivered: QML's animation timer pauses on it.
            crate::qml::tick(now());
            scene.set_bool("first", true);
            let mut frames = 0_u32;
            while scene.get_int("led") < 1000 && frames < 60 {
                std::thread::sleep(Duration::from_millis(20));
                crate::qml::tick(now());
                frames += 1;
            }
            let ended = scene.get_int("led");
            // The first frame after the one the old animation ended on.
            scene.set_bool("second", true);
            let mut shown = Vec::new();
            for _ in 0..2 {
                std::thread::sleep(Duration::from_millis(20));
                crate::qml::tick(now());
                shown.push(scene.get_int("permille"));
            }

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                ended, 1000,
                "the control failed: the first animation had not ended after {frames} frames"
            );
            assert!(
                shown.iter().all(|permille| *permille < 150),
                "the two frames after a one-second animation started, on the frame after a \
                 300 ms one ended, showed it at {shown:?} permille: it was handed the old one"
            );
        });
    }

    /// **A `Timer` keeps firing beside an animation that is never drawn.**
    ///
    /// While any animation runs, Qt moves every Timer onto the animation
    /// driver, and the driver used to be advanced only by frames. So an
    /// animation still running in a scene nobody draws -- a hidden popup, a
    /// decoration on a workspace that is not shown, every screen off -- stopped
    /// every Timer in the process. Here the animation never ends, nothing is
    /// drawn for a second, and a 100 ms Timer must still fire about ten times,
    /// on wakes a frame apart rather than a spinning loop.
    #[test]
    fn a_timer_beside_an_undrawn_animation_fires_with_no_frame_drawn() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-hidden",
                r"
                import QtQuick

                Item {
                    id: root
                    property int fired: 0
                    property real turn: 0
                    NumberAnimation on turn {
                        from: 0
                        to: 360
                        duration: 1000
                        loops: Animation.Infinite
                    }
                    Timer {
                        interval: 100
                        running: true
                        repeat: true
                        onTriggered: root.fired += 1
                    }
                }
                ",
            );

            let seen = run(Duration::from_secs(1), || {});
            let fired = scene.get_int("fired");
            assert!(
                (8..=10).contains(&fired),
                "a 100 ms Timer beside an undrawn animation fired {fired} times in a second: \
                 {seen:?}"
            );
            // Never drawn, so it was waiting for its first frame all along.
            assert_eq!(
                seen.changed, 0,
                "wakes asked to draw a scene that was already waiting: {seen:?}"
            );
            // A step a frame, 62 in the second, and a wake for each firing's
            // posted tick: the clock stepped as Qt's own driver would step it,
            // and not a loop spinning on an animation nobody draws.
            assert!(
                seen.wakes <= 100,
                "an undrawn animation woke the loop {} times in a second: {seen:?}",
                seen.wakes
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A `Timer` keeps firing beside a transition that is never drawn, and
    /// the transition moves.**
    ///
    /// The animations inside a `Transition` never say they are running, so a
    /// walk that asked only them saw nothing animate while the transition held
    /// every Timer on the animation clock: no step was taken between frames,
    /// the clock's origin was dragged along behind it, and the Timer and the
    /// transition both stood still, drawn or not. Here a state change starts a
    /// one-second transition in a scene nothing draws, beside a 100 ms Timer.
    /// Over a second with no frame the Timer must fire about ten times, and
    /// the value must be well on its way.
    #[test]
    fn a_timer_beside_an_undrawn_transition_fires_and_the_transition_moves() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-transition",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool go: false
                    property int fired: 0
                    property real value: 0
                    readonly property int permille: Math.round(root.value * 1000)
                    states: State {
                        name: 'there'
                        when: root.go
                        PropertyChanges {
                            target: root
                            value: 1
                        }
                    }
                    transitions: Transition {
                        NumberAnimation {
                            property: 'value'
                            duration: 1000
                        }
                    }
                    Timer {
                        interval: 100
                        running: true
                        repeat: true
                        onTriggered: root.fired += 1
                    }
                }
                ",
            );

            scene.set_bool("go", true);
            let seen = run(Duration::from_secs(1), || {});
            let fired = scene.get_int("fired");
            let permille = scene.get_int("permille");

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert!(
                (8..=10).contains(&fired),
                "a 100 ms Timer beside an undrawn transition fired {fired} times in a second: \
                 {seen:?}"
            );
            assert!(
                permille >= 500,
                "a one-second transition nothing draws was {permille}/1000 of the way after a \
                 second: {seen:?}"
            );
        });
    }

    /// **A transition moves on the frames that draw it.**
    ///
    /// The same blindness on the drawn path: with nothing counted as animating,
    /// every frame's tick dragged the clock's origin up to the frame, so the
    /// clock read zero on every frame and the transition never took a step,
    /// however many frames were drawn. Here thirty frames 16 ms apart follow
    /// the state change, and the one-second transition must be at least 300
    /// of its 1000 permille on its way. The scene must also say it is
    /// animating as the transition starts: the render loop asks it on a clean
    /// frame, and a "no" there asks for no more frames.
    #[test]
    fn a_transition_moves_on_the_frames_that_draw_it() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-transition-drawn",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool go: false
                    property real value: 0
                    readonly property int permille: Math.round(root.value * 1000)
                    states: State {
                        name: 'there'
                        when: root.go
                        PropertyChanges {
                            target: root
                            value: 1
                        }
                    }
                    transitions: Transition {
                        NumberAnimation {
                            property: 'value'
                            duration: 1000
                        }
                    }
                }
                ",
            );

            scene.set_bool("go", true);
            let walked = scene.animation_in_flight();
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(16));
                crate::qml::tick(now());
            }
            let permille = scene.get_int("permille");

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert!(
                walked,
                "a scene whose transition had just started did not say it was animating"
            );
            assert!(
                permille >= 300,
                "a one-second transition was {permille}/1000 of the way after thirty frames"
            );
        });
    }

    /// **A `Timer` keeps firing beside a flick that is never drawn, and the
    /// flick moves.**
    ///
    /// A Flickable -- every ListView -- moves on a timeline of its own, which
    /// is an animation job but no animation object a walk can find. Flicked in
    /// a scene nothing draws, it held every Timer on the animation clock with
    /// no step taken, and stood still itself. Here one is flicked at 2000 px/s
    /// beside a 100 ms Timer; over a second with no frame the Timer must fire
    /// about ten times and the content must have travelled at least 500 px of
    /// the 1250 a second covers at Qt's default deceleration, 1500 px/s²
    /// (qtbase v6.11.2, src/gui/kernel/qplatformtheme.cpp:701-702). The scene
    /// must also say it is animating as the flick starts, for the render loop.
    #[test]
    fn a_timer_beside_an_undrawn_flick_fires_and_the_flick_moves() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-flick",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool go: false
                    property int fired: 0
                    readonly property int scrolled: Math.round(list.contentY)
                    Flickable {
                        id: list
                        width: 64
                        height: 16
                        contentWidth: 64
                        contentHeight: 100000
                    }
                    onGoChanged: if (root.go) list.flick(0, -2000)
                    Timer {
                        interval: 100
                        running: true
                        repeat: true
                        onTriggered: root.fired += 1
                    }
                }
                ",
            );

            scene.set_bool("go", true);
            let walked = scene.animation_in_flight();
            let seen = run(Duration::from_secs(1), || {});
            let fired = scene.get_int("fired");
            let scrolled = scene.get_int("scrolled");

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert!(
                walked,
                "a scene whose Flickable had just been flicked did not say it was animating"
            );
            assert!(
                (8..=10).contains(&fired),
                "a 100 ms Timer beside an undrawn flick fired {fired} times in a second: {seen:?}"
            );
            assert!(
                scrolled >= 500,
                "a flick at 2000 px/s nothing draws had moved {scrolled} px after a second: \
                 {seen:?}"
            );
        });
    }

    /// **A `Timer` keeps firing beside an animation no scene holds.**
    ///
    /// No walk of the scenes reaches everything that runs on the animation
    /// clock: an animation built with no parent, or one in a singleton. Such
    /// an animation holds every Timer on the clock just the same. Here one is
    /// built with `createObject(null)` and never ends, beside a 100 ms Timer;
    /// the walk must not see it -- the control -- and over a second with no
    /// frame the Timer must still fire about ten times, on wakes a frame apart
    /// rather than a spinning loop.
    #[test]
    fn a_timer_beside_an_animation_no_scene_holds_fires_with_no_frame_drawn() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-orphan",
                r"
                import QtQuick

                Item {
                    id: root
                    property int fired: 0
                    property real turn: 0
                    property bool done: false
                    property var spinner: null
                    Component {
                        id: spinning
                        NumberAnimation {
                            target: root
                            property: 'turn'
                            from: 0
                            to: 360
                            duration: 1000
                            loops: Animation.Infinite
                            running: true
                        }
                    }
                    Component.onCompleted: root.spinner = spinning.createObject(null)
                    onDoneChanged: if (root.done) {
                        root.spinner.running = false
                        root.spinner.destroy()
                    }
                    Timer {
                        interval: 100
                        running: true
                        repeat: true
                        onTriggered: root.fired += 1
                    }
                }
                ",
            );

            let walked = scene.animation_in_flight();
            let seen = run(Duration::from_secs(1), || {});
            let fired = scene.get_int("fired");

            // The animation outlives the scene unless it is stopped here, and
            // would hold every later test's Timers on the clock.
            scene.set_bool("done", true);
            let _ = run(Duration::from_millis(100), || {});
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert!(
                !walked,
                "the control failed: a walk of the scene reached the animation built with no parent"
            );
            assert!(
                (8..=10).contains(&fired),
                "a 100 ms Timer beside an animation no scene holds fired {fired} times in a \
                 second: {seen:?}"
            );
            assert!(
                seen.wakes <= 100,
                "an animation no scene holds woke the loop {} times in a second: {seen:?}",
                seen.wakes
            );
        });
    }

    /// **A `Timer` in a singleton keeps firing beside an animation no scene
    /// holds.**
    ///
    /// Most of a shell's Timers are in `pragma Singleton` services -- a clock,
    /// notification expiry -- where no walk of the scenes reaches them, and an
    /// animation built with no parent holds every one of them on the animation
    /// clock. A clock that asked the scenes whether to step found no animation
    /// and no Timer, stepped nothing, and the singleton's Timer never fired.
    /// Here no scene holds a Timer, a singleton holds a 100 ms one, and an
    /// endless animation is built with `createObject(null)`; over a second
    /// with no frame the Timer must fire about ten times, on wakes a frame
    /// apart rather than a spinning loop.
    #[test]
    fn a_singleton_timer_beside_a_parentless_animation_fires_with_no_frame_drawn() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build_beside(
                "solium-qml-test-wake-singleton",
                &[
                    ("qmldir", "singleton Beat 1.0 Beat.qml\n"),
                    (
                        "Beat.qml",
                        r"
                        pragma Singleton
                        import QtQuick

                        Item {
                            id: beat
                            property bool on: true
                            property int fired: 0
                            Timer {
                                interval: 100
                                running: beat.on
                                repeat: true
                                onTriggered: beat.fired += 1
                            }
                        }
                        ",
                    ),
                ],
                r#"
                import QtQuick
                import "."

                Item {
                    id: root
                    readonly property int fired: Beat.fired
                    property real turn: 0
                    property bool done: false
                    property var spinner: null
                    Component {
                        id: spinning
                        NumberAnimation {
                            target: root
                            property: 'turn'
                            from: 0
                            to: 360
                            duration: 1000
                            loops: Animation.Infinite
                            running: true
                        }
                    }
                    Component.onCompleted: root.spinner = spinning.createObject(null)
                    onDoneChanged: if (root.done) {
                        Beat.on = false
                        root.spinner.running = false
                        root.spinner.destroy()
                    }
                }
                "#,
            );

            let walked = scene.animation_in_flight();
            let seen = run(Duration::from_secs(1), || {});
            let fired = scene.get_int("fired");

            // Both outlive the scene unless they are stopped here: the
            // animation would hold every later test's Timers on the clock, and
            // the Timer would wake every later test's idle loop.
            scene.set_bool("done", true);
            let _ = run(Duration::from_millis(100), || {});
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert!(
                !walked,
                "the control failed: a walk of the scene reached the animation built with no \
                 parent"
            );
            assert!(
                (8..=10).contains(&fired),
                "a 100 ms Timer in a singleton, beside an animation no scene holds, fired {fired} \
                 times in a second: {seen:?}"
            );
            assert!(
                seen.wakes <= 100,
                "an animation no scene holds woke the loop {} times in a second: {seen:?}",
                seen.wakes
            );
        });
    }

    /// **A ListView's highlight moves on the frames that draw it**, with
    /// nothing else animating.
    ///
    /// The highlight follows the current item on a job the view keeps to
    /// itself, where no walk of the scenes reaches it. A clock that asked the
    /// scenes whether anything was animating found nothing, dragged its origin
    /// up to every frame and read zero on each, so the highlight never took a
    /// step however many frames were drawn: an arrow key in a launcher moved
    /// nothing. Here the current item moves nine rows, 144 px, over a second,
    /// and after thirty frames 16 ms apart the highlight must be at least
    /// 20 px on its way.
    #[test]
    fn a_list_highlight_moves_on_the_frames_that_draw_it() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-highlight",
                r"
                import QtQuick

                Item {
                    id: root
                    property bool go: false
                    readonly property int travelled:
                        list.highlightItem ? Math.round(list.highlightItem.y) : -1
                    ListView {
                        id: list
                        width: 64
                        height: 160
                        model: 10
                        delegate: Item {
                            width: 64
                            height: 16
                        }
                        highlight: Item {}
                        highlightMoveDuration: 1000
                        highlightMoveVelocity: -1
                        currentIndex: root.go ? 9 : 0
                    }
                }
                ",
            );
            // The frame that first shows it, with the view laid out.
            assert!(
                scene.render().expect("the first frame").changed,
                "a new scene had nothing to draw"
            );
            let before = scene.get_int("travelled");
            scene.set_bool("go", true);
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(16));
                crate::qml::tick(now());
            }
            let travelled = scene.get_int("travelled");

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                before, 0,
                "the control failed: the highlight did not start on the first row"
            );
            assert!(
                travelled >= 20,
                "a highlight moving 144 px over a second was {travelled} px on its way after \
                 thirty frames"
            );
        });
    }

    /// **A descriptor Qt waits on reaches its scene while no frame is drawn**,
    /// with no `Timer` in the scene, and the loop is quiet again afterwards.
    ///
    /// The host watches one end of a socket pair with a `QSocketNotifier`,
    /// which is how Qt waits on a child's pipes, a local socket or eglfs's
    /// signal pipe. Nothing is due and nothing draws, so only a loop that
    /// watches Qt's own descriptors wakes when the other end is written, 200 ms
    /// in. The handler must run within 50 ms of the write, and the count it
    /// writes is drawn, so the scene must ask for its frame, once.
    ///
    /// Then the writer hangs up. With the byte read and the end of file seen,
    /// and the wake-up GLib sends itself when a descriptor leaves its set
    /// served, a second of the loop must be one wait: a descriptor left ready,
    /// or GLib's wake-up descriptor left readable, would make it spin.
    #[test]
    fn a_ready_descriptor_reaches_its_scene_with_no_frame_drawn() {
        use std::io::Write as _;
        use std::os::fd::AsFd as _;

        on_the_qt_thread(|| {
            // Made before the scene, so that the scene, and the notifier it
            // owns, goes first even when an assertion fails.
            let (watched, mut writer) =
                std::os::unix::net::UnixStream::pair().expect("a socket pair");
            watched
                .set_nonblocking(true)
                .expect("a non-blocking socket");
            let (directory, mut scene) = build(
                "solium-qml-test-wake-notifier",
                r"
                import QtQuick

                Item {
                    id: root
                    property int received: 0
                    Text {
                        text: root.received
                    }
                }
                ",
            );
            assert!(
                scene.watch_for_test(watched.as_fd(), "received"),
                "the host would not watch the socket"
            );
            // The scene is dirty from birth; the frame that first shows it.
            assert!(
                scene.render().expect("the first frame").changed,
                "a new scene had nothing to draw"
            );

            let writing = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                let written = Instant::now();
                writer
                    .write_all(b"x")
                    .expect("writing to the watched socket");
                drop(writer);
                written
            });
            let mut handled = None;
            let busy = run_until(
                Duration::from_secs(2),
                || {
                    if handled.is_none() && scene.get_int("received") > 0 {
                        handled = Some(Instant::now());
                    }
                    handled.is_some()
                },
                || {},
            );
            let written = writing.join().expect("the writing thread");
            let lag = handled.map(|at| at.saturating_duration_since(written));
            assert!(
                lag.is_some_and(|lag| lag < Duration::from_millis(50)),
                "the watched socket's handler ran {lag:?} after the write (None: never): {busy:?}"
            );
            assert_eq!(
                busy.changed, 1,
                "what the handler wrote did not ask for its frame, once: {busy:?}"
            );

            // The end of file, and GLib's own wake-up for the notifier leaving.
            let settling = run(Duration::from_millis(200), || {});
            let idle = run(Duration::from_secs(1), || {});
            assert!(
                idle.wakes <= 1 && idle.iterations <= 2,
                "with the byte read and the writer gone the loop still woke: {idle:?} \
                 (settling: {settling:?}, before: {busy:?})"
            );
            assert_eq!(
                scene.get_int("received"),
                1,
                "the handler counted bytes nobody wrote"
            );

            drop(scene);
            drop(watched);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **An event another thread posts to Qt's thread is delivered while no
    /// frame is drawn**, and the loop is quiet again afterwards.
    ///
    /// A `WorkerScript` runs on a thread of its own and answers by posting an
    /// event to the scene's thread, which is how an asynchronous image load,
    /// the QML loader thread and a network reply all come back too. Posting
    /// wakes Qt's dispatcher through a descriptor of its own, not a timer. The
    /// worker waits 200 ms before it answers, so the answer lands while the
    /// loop sleeps, and it must be in the scene within 50 ms. Then, with
    /// nothing left to deliver, a second of the loop must be one wait: a
    /// wake-up descriptor left readable would make it spin.
    #[test]
    fn an_event_posted_from_another_thread_is_delivered_with_no_frame_drawn() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build_beside(
                "solium-qml-test-wake-posted",
                &[(
                    "worker.js",
                    r"
                    WorkerScript.onMessage = function (message) {
                        const until = Date.now() + 200
                        while (Date.now() < until) {}
                        WorkerScript.sendMessage({ sent: Date.now() })
                    }
                    ",
                )],
                r"
                import QtQuick

                Item {
                    id: root
                    property int lag: -1
                    property bool answered: false
                    WorkerScript {
                        id: worker
                        source: 'worker.js'
                        onReadyChanged: if (ready) worker.sendMessage({})
                        onMessage: message => {
                            root.lag = Date.now() - message.sent
                            root.answered = true
                        }
                    }
                }
                ",
            );

            let busy = run_until(Duration::from_secs(3), || scene.get_bool("answered"), || {});
            let lag = scene.get_int("lag");
            assert!(
                (0..50).contains(&lag),
                "the worker's answer reached the scene {lag} ms after it was sent (-1: never): \
                 {busy:?}"
            );

            let idle = run(Duration::from_secs(1), || {});
            assert!(
                idle.wakes <= 1 && idle.iterations <= 2,
                "with the answer delivered the loop still woke: {idle:?} (before: {busy:?})"
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A drain while a frame is coming delivers, and leaves the step to the
    /// frame.**
    ///
    /// The frame's own tick advances every animation to its `now`, so a wake
    /// just before it that stepped them too would take a step the frame then
    /// takes again, only shorter. Asked not to advance, a drain moves nothing;
    /// asked to, it moves an endless animation by the time that has passed.
    #[test]
    fn a_drain_before_a_frame_leaves_the_step_to_it() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-coming",
                r"
                import QtQuick

                Item {
                    property int spin: 0
                    NumberAnimation on spin {
                        from: 0
                        to: 3600000
                        duration: 3600000
                        loops: Animation.Infinite
                    }
                }
                ",
            );
            // Registered, which is a posted event, and stepped once.
            for _ in 0..3 {
                crate::qml::drain(now(), true);
            }
            let before = scene.get_int("spin");
            std::thread::sleep(Duration::from_millis(50));
            crate::qml::drain(now(), false);
            let delivered = scene.get_int("spin");
            assert_eq!(
                delivered, before,
                "a drain with a frame coming stepped the animation from {before} to {delivered}"
            );
            crate::qml::drain(now(), true);
            let stepped = scene.get_int("spin");
            assert!(
                stepped >= before + 40,
                "a drain with no frame coming did not step the animation: {before} -> {stepped}"
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **While a frame is coming, the frame advances the clock**, and the poll
    /// set does not wake to do it first.
    ///
    /// The same endless animation as above, with the backend saying a frame
    /// is on its way throughout. Nothing may step it, and nothing may wake the
    /// loop for it: stepping here as well would be every animation advanced
    /// twice between two frames.
    #[test]
    fn a_coming_frame_is_left_to_advance_the_clock() {
        on_the_qt_thread(|| {
            let (directory, mut scene) = build(
                "solium-qml-test-wake-left",
                r"
                import QtQuick

                Item {
                    property int spin: 0
                    NumberAnimation on spin {
                        from: 0
                        to: 3600000
                        duration: 3600000
                        loops: Animation.Infinite
                    }
                }
                ",
            );
            // Registered, which is a posted event.
            let _ = run_with(Duration::from_millis(100), true, || false, || {});
            let before = scene.get_int("spin");
            let seen = run_with(Duration::from_millis(500), true, || false, || {});
            let after = scene.get_int("spin");
            assert_eq!(
                after, before,
                "the poll set stepped an animation a coming frame was to step: {seen:?}"
            );
            assert!(
                seen.wakes <= 1,
                "the poll set woke for an animation a coming frame was to step: {seen:?}"
            );

            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A descriptor Qt stops watching is let go, even while it stays open.**
    ///
    /// GLib closes a descriptor it is done with, and the file under it can
    /// outlive that: a dup, a child that inherited it. An epoll entry lives as
    /// long as the file does and cannot be deleted through a closed
    /// descriptor, so a set edited one entry at a time would keep reporting
    /// this one, readable and level-triggered, to a loop with nobody left to
    /// drain it. Rebuilt from the query, the set forgets it.
    #[test]
    fn a_descriptor_qt_stops_watching_is_let_go_while_it_stays_open() {
        use std::io::Write as _;
        use std::os::fd::AsRawFd as _;

        let timer = rustix::time::timerfd_create(
            super::TimerfdClockId::Monotonic,
            super::TimerfdFlags::CLOEXEC | super::TimerfdFlags::NONBLOCK,
        )
        .expect("a timer");
        let mut set = super::PollSet::new(&timer).expect("a poll set");

        let (glib, mut peer) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
        peer.write_all(b"x").expect("writing to it");
        let elsewhere = glib
            .try_clone()
            .expect("a second descriptor for the same file");
        set.watch(&[super::PollFd {
            fd: glib.as_raw_fd(),
            events: super::G_IO_IN,
            _revents: 0,
        }]);
        assert!(
            readable(&set),
            "the control failed: a readable socket did not wake the set"
        );

        drop(glib);
        set.watch(&[]);
        assert!(
            !readable(&set),
            "a descriptor Qt no longer watches still wakes the loop while its file is open"
        );
        drop(elsewhere);
    }

    /// **A descriptor number reused for another file is watched.**
    ///
    /// The query names the same number with the same events before and after,
    /// but the file under it is new: a pipe closed and a new one opened in its
    /// place, as a child process restarted from its own exit handler does
    /// within one drain. The old file's entry went with it, so a set kept
    /// because the query had not changed would watch nothing there.
    #[test]
    fn a_descriptor_number_reused_for_another_file_is_watched() {
        use std::io::Write as _;
        use std::os::fd::AsRawFd as _;

        let timer = rustix::time::timerfd_create(
            super::TimerfdClockId::Monotonic,
            super::TimerfdFlags::CLOEXEC | super::TimerfdFlags::NONBLOCK,
        )
        .expect("a timer");
        let mut set = super::PollSet::new(&timer).expect("a poll set");

        let (first, _first_peer) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
        let mut number = std::os::fd::OwnedFd::from(first);
        let query = [super::PollFd {
            fd: number.as_raw_fd(),
            events: super::G_IO_IN,
            _revents: 0,
        }];
        set.watch(&query);
        assert!(
            !readable(&set),
            "the control failed: an empty socket woke the set"
        );

        let (second, mut second_peer) =
            std::os::unix::net::UnixStream::pair().expect("a second socket pair");
        rustix::io::dup2(&second, &mut number).expect("reusing the number");
        drop(second);
        second_peer.write_all(b"x").expect("writing to it");
        set.watch(&query);
        assert!(
            readable(&set),
            "a descriptor number now naming another file, readable, did not wake the set"
        );
    }

    /// **A frame is coming only if one is wanted and a monitor will draw it.**
    ///
    /// The frame's tick is what advances the clock while it comes, so saying
    /// yes for a frame that is never drawn is every Timer frozen again: every
    /// screen off (#54), or a black frame on its way there. A monitor still
    /// flipping counts, because its flip asks again.
    #[test]
    fn a_frame_is_coming_only_if_a_monitor_will_draw_it() {
        use super::frame_coming;
        use crate::power::Step;

        assert!(frame_coming(true, [Some(Step::Draw)]));
        assert!(frame_coming(true, [Some(Step::Rest), Some(Step::Wake)]));
        assert!(
            frame_coming(true, [None, Some(Step::Rest)]),
            "a flip is pending"
        );
        assert!(
            !frame_coming(false, [Some(Step::Draw), None]),
            "nothing wants one"
        );
        assert!(
            !frame_coming(true, [Some(Step::Rest), Some(Step::Rest)]),
            "every screen off"
        );
        assert!(
            !frame_coming(true, [Some(Step::Blank), Some(Step::Darken)]),
            "black frames tick nothing"
        );
        assert!(!frame_coming(true, []), "no monitor at all");
    }
}
