//! Qt's timers, served between frames (#164).
//!
//! [`super::tick`] drains Qt's events only on a frame that is drawn, so on an
//! idle desktop a QML `Timer` never fired and a hosted clock stood still. This
//! holds one timerfd at the deadline Qt itself reports ([`super::next_due`]),
//! registered once with the compositor's event loop. When it fires, Qt's due
//! timers and posted events are delivered ([`super::drain`]), and a scene that
//! changed asks for one frame through the backend's ordinary `redraw` flag.
//! Nothing due, nothing armed: `tests::an_idle_host_does_not_wake_repeatedly`.

use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use smithay::reexports::{
    calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic},
    rustix::{
        self,
        time::{Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec},
    },
};

/// [`super::next_due`] is whole milliseconds, so deadlines closer than one
/// are the same deadline.
const SLACK: Duration = Duration::from_millis(1);

/// The timer Qt's next due work is held on.
#[derive(Debug)]
pub(crate) struct Wake {
    /// A second descriptor for the timer the event loop polls, kept to arm it.
    timer: OwnedFd,
    /// Where the timer was last pointed, or `None` while it is disarmed.
    armed: Option<Instant>,
}

impl Wake {
    /// Put the timer in `handle`'s loop. `woke` is told, on every wake,
    /// whether delivering Qt's events turned a clean scene dirty.
    pub(crate) fn insert<D: 'static>(
        handle: &LoopHandle<'_, D>,
        mut woke: impl FnMut(&mut D, bool) + 'static,
    ) -> Result<Self> {
        let timer = rustix::time::timerfd_create(
            TimerfdClockId::Monotonic,
            TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
        )
        .map_err(|err| anyhow!("creating the timer Qt's timers wake on: {err}"))?;
        let polled = timer
            .try_clone()
            .map_err(|err| anyhow!("duplicating the timer Qt's timers wake on: {err}"))?;
        handle
            .insert_source(
                Generic::new(polled, Interest::READ, Mode::Level),
                move |_, timer, data| {
                    let mut expirations = [0_u8; 8];
                    let _ = rustix::io::read(&*timer, &mut expirations[..]);
                    woke(data, super::drain());
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| anyhow!("watching the timer Qt's timers wake on: {err}"))?;
        Ok(Self { timer, armed: None })
    }

    /// Point the timer at Qt's next due work, or disarm it.
    ///
    /// Once per loop iteration, after the frame and before the loop sleeps:
    /// anything that ran in the iteration may have started or stopped a Qt
    /// timer. `tests::an_idle_host_does_not_wake_repeatedly`.
    pub(crate) fn arm(&mut self) {
        let now = Instant::now();
        let wanted = super::next_due().and_then(|due| now.checked_add(due));
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

    use smithay::reexports::calloop::EventLoop;

    use super::Wake;
    use crate::qml::{Scene, qt_test::on_the_qt_thread};

    /// What one window of the loop saw.
    #[derive(Debug, Default)]
    struct Seen {
        /// Times the loop came back from waiting.
        iterations: u32,
        /// Times Qt's timer was what woke it.
        wakes: u32,
        /// Wakes that turned a clean scene dirty.
        changed: u32,
        redraw: bool,
    }

    /// The compositor's loop with nothing in it but Qt's timer: no clients, no
    /// input, and no [`crate::qml::tick`], so no frame and no animation clock.
    /// `draw` stands in for the frame a backend draws when `redraw` is set.
    fn run(window: Duration, mut draw: impl FnMut()) -> Seen {
        let mut event_loop: EventLoop<Seen> = EventLoop::try_new().expect("an event loop");
        let mut wake = Wake::insert(&event_loop.handle(), |seen: &mut Seen, changed| {
            seen.wakes += 1;
            if changed {
                seen.changed += 1;
                seen.redraw = true;
            }
        })
        .expect("Qt's wake-up timer");
        let mut seen = Seen::default();
        let start = Instant::now();
        loop {
            if std::mem::take(&mut seen.redraw) {
                draw();
            }
            wake.arm();
            let left = window.saturating_sub(start.elapsed());
            if left.is_zero() {
                return seen;
            }
            event_loop
                .dispatch(Some(left), &mut seen)
                .expect("dispatching the loop");
            seen.iterations += 1;
        }
    }

    /// A scene built from `qml`, the way a hosted shell's is, in a fresh
    /// directory that the caller removes.
    fn build(name: &str, qml: &str) -> (PathBuf, Scene) {
        let directory = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
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
}
