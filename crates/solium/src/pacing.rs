//! Where a frame's time went, on the frames that did not fit in one.
//!
//! The compositor could already say *that* it was slow — `winit.rs` reports an
//! average frame rate every two seconds — and nothing anywhere could say
//! **why**. "Sometimes everything lags, sometimes it is perfect" is not a
//! report anybody can act on, and the reason it stays unactionable is that the
//! interesting frame is over before anyone can look at it. So this measures
//! every frame and keeps its mouth shut about almost all of them.
//!
//! Three properties, in the order they constrain the design:
//!
//! * **It has to be free when off.** A hardware session runs at the monitor's
//!   refresh — 260 Hz on the desk this was written for — so anything paid per
//!   frame is paid 260 times a second, and anything paid per *scene* per output
//!   per frame is paid thousands of times a second. Off, every call here is one
//!   non-atomic thread-local load and a predictable branch; nothing samples a
//!   clock, nothing allocates, nothing formats. It is deliberately not a
//!   compile-time feature: a diagnostic that is not in the shipped binary is
//!   not there on the day the shipped binary is slow, which is the only day it
//!   is wanted.
//!
//! * **It has to be nearly free when on.** One `Instant::now` — a vDSO
//!   `clock_gettime` on this platform, tens of nanoseconds — per phase
//!   boundary, into a fixed array of `Cell<u64>` nanosecond counters. No
//!   allocation on the per-frame path except the monitor's name, which the
//!   backend takes once a frame and only when the knob is on.
//!
//! * **It must not become the lag it is looking for.** The log is opened
//!   `O_DSYNC` (see `open_log`), which makes every line a synchronous write to
//!   disk — deliberately, so a session that ends in the power button still has
//!   its last seconds. A line per frame at 260 Hz is 260 synchronous writes a
//!   second and would comfortably out-stall anything it was measuring. See
//!   [`REPORT_EVERY`].
//!
//! # What a phase is
//!
//! Exclusive, not nested. Entering a phase suspends whichever one was running
//! and resumes it on the way out, so the numbers add up to the frame rather
//! than over-counting it — `qml` happens inside `elements`, and `elements` is
//! reported without it. That is the whole point: a frame that spent 7 ms
//! collecting elements is a compositor problem and a frame that spent 7 ms in
//! Qt underneath the same call is not, and a single nested number cannot tell
//! them apart.
//!
//! [`Phase::Loose`] catches whatever no phase claimed. It is not padding: a
//! large `loose` says the breakdown has a hole in it and the next person should
//! go and find what moved.

use std::{
    cell::{Cell, RefCell},
    time::{Duration, Instant},
};

/// How often a report may be made, at most.
///
/// **One second**, and the reasoning is the log rather than the measurement.
/// `open_log` opens `session.log` with `O_DSYNC`, so a line is a synchronous
/// write; at 260 Hz a line per slow frame during a sustained stall is 260
/// synchronous writes a second, which is a diagnostic that causes the thing it
/// reports. One a second is the rate `SOLIUM_MEMDIAG` already runs at and is
/// accepted at, and it is two orders of magnitude under journald's own default
/// (`RateLimitBurst=10000` per `RateLimitIntervalSec=30s`) — so this cannot be
/// the thing that makes the journal start dropping *other* messages, which is
/// how a flood buries its own cause.
///
/// Nothing is lost by the limit, which is the part worth getting right. A
/// report is not "the frame that happened to be slow when the timer fired": it
/// carries the **worst** frame since the last report, with its full breakdown,
/// plus how many frames missed out of how many were drawn and over what span.
/// So a single hiccup reads `missed=1 frames=3` and a sustained stall reads
/// `missed=247 frames=259`, and the two are never confusable.
///
/// The first miss after a quiet spell reports immediately rather than waiting
/// out a window — a diagnostic whose first evidence arrives a second late is
/// hard to trust, and it is the case somebody is watching the log live for.
const REPORT_EVERY: Duration = Duration::from_secs(1);

/// The parts of a frame, each measured exclusively of the others.
///
/// Chosen so that a reader can attribute a slow frame to a *culprit* rather
/// than to a call stack. The three that matter are Qt, the driver and the
/// compositor, and every phase here belongs to exactly one of them:
///
/// | phase | who owns it |
/// |---|---|
/// | [`Tick`](Phase::Tick) | Qt |
/// | [`Census`](Phase::Census) | Qt |
/// | [`Qml`](Phase::Qml) | Qt, and the driver underneath it |
/// | [`Prep`](Phase::Prep) | the compositor |
/// | [`Elements`](Phase::Elements) | the compositor |
/// | [`Gles`](Phase::Gles) | the driver |
/// | [`Commit`](Phase::Commit) | the kernel, and the driver under it |
/// | [`Settle`](Phase::Settle) | the compositor |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// `qml::tick`: every animation in the process advanced by one step, and
    /// Qt's event queue drained. Once per frame, never once per output.
    Tick,
    /// The rest of `render::prepare`, plus `screencopy::settle`: the window
    /// list published to the shell, and one offscreen render pass for every
    /// window whose transform is not a rectangle.
    Prep,
    /// `Painted::animation_in_flight` — the walk of a scene's QML object tree
    /// looking for a running animation, asked once per scene per output per
    /// frame. Measured at ~0.6 µs for a settled 21-object scene, which is
    /// exactly the kind of number that is fine until it is multiplied by
    /// scenes, by outputs and by 260.
    Census,
    /// Qt rendering a scene and the compositor getting the result: on the GPU
    /// path a rebind, a render, a fence wait and an `import_dmabuf`; on the
    /// software path the rasterisation into Qt's `QImage`. The copy out of that
    /// image is the compositor's and lands in [`Elements`](Phase::Elements);
    /// the texture upload is the driver's and lands in [`Gles`](Phase::Gles).
    Qml,
    /// What is left of `render::elements` once Qt's share is taken out of it:
    /// walking the panes, the layer maps and the popups, and building Smithay's
    /// render elements. Summed over every output drawn this frame.
    Elements,
    /// `render_frame` / `render_output`: damage tracking and the GL draw calls.
    /// Summed over every output drawn this frame.
    Gles,
    /// `queue_frame` on the hardware — the DRM atomic commit — and `submit` on
    /// the nested backend. Summed over every output drawn this frame.
    Commit,
    /// `Solium::settle`: retiring the transforms that have landed and deciding
    /// whether anything still needs the frame after this one.
    Settle,
    /// Time inside a frame that no phase claimed.
    ///
    /// Reported rather than hidden. A large `loose` does not mean the frame was
    /// fine — it means this breakdown has a hole in it and somebody should find
    /// out what moved into the gap.
    Loose,
}

impl Phase {
    /// How many there are, which is the width of the counter array.
    const COUNT: usize = 9;

    const fn slot(self) -> usize {
        self as usize
    }
}

/// One frame's measurements, kept for as long as it is the worst one seen.
///
/// A snapshot rather than a borrow of the live state, because the live state is
/// about to be reused by the next frame and this one may not be reported for
/// another second.
#[derive(Clone, Debug)]
struct Slow {
    total: Duration,
    deadline: Duration,
    monitor: String,
    spent: [u64; Phase::COUNT],
    panes: u32,
    drew: u32,
    scenes: u32,
    animating: u32,
    rendered: u32,
    rebound: u32,
}

/// Everything this module holds, for one thread.
///
/// `Cell` throughout on the hot path rather than a `RefCell`: the phase switch
/// runs tens of times a frame and `RefCell` can panic, which the workspace
/// denies for good reason — a compositor crash takes the session with it, and a
/// diagnostic that can end a session is strictly worse than no diagnostic. The
/// two `RefCell`s left hold strings, are touched at most once a frame, and are
/// only ever entered with `try_borrow_mut`.
///
/// Thread-local rather than threaded through the render path as an argument.
/// That is a real trade and it is made on purpose: the phases are entered from
/// `tty.rs`, `winit.rs`, `render.rs`, `qml.rs` and `qml/paint.rs`, and
/// threading a `&mut` through all of them would change the signature of every
/// function on the render path — on a branch other sessions are committing to.
/// The compositor's render loop is single-threaded, which is the same fact
/// `qml::FRAMES_IN_FLIGHT` already depends on; a second thread would simply get
/// its own counters and never report, rather than corrupt these.
struct Counters {
    /// Whether the knob is on. Read from the environment once, ever.
    on: Cell<bool>,
    /// Whether `on` has been decided yet.
    asked: Cell<bool>,
    /// Whether a frame is being measured right now.
    ///
    /// The flag every call site tests. Separate from `on` because scenes are
    /// rendered outside a frame too — the GPU pre-flight at startup, a scene
    /// built lazily — and time spent there belongs to nothing.
    live: Cell<bool>,
    /// When the frame being measured began.
    started: Cell<Option<Instant>>,
    /// When the phase currently running began.
    mark: Cell<Option<Instant>>,
    /// Which phase is running.
    phase: Cell<Phase>,
    /// Nanoseconds spent in each phase of the frame being measured.
    spent: [Cell<u64>; Phase::COUNT],

    /// How many scenes were asked whether they are animating.
    scenes: Cell<u32>,
    /// How many of them said yes.
    animating: Cell<u32>,
    /// How many QML scenes actually re-rendered, rather than being served from
    /// a cache or skipped as clean.
    rendered: Cell<u32>,
    /// How many scenes were rebound onto a new buffer — the expensive,
    /// normally invisible case: a GBM allocation, an `eglCreateImageKHR`, a Qt
    /// render-target swap and a full Qt render.
    rebound: Cell<u32>,
    /// How many outputs were actually drawn.
    drew: Cell<u32>,
    /// The tightest frame interval among the monitors being driven.
    deadline: Cell<Duration>,
    /// Which monitor that interval belongs to.
    monitor: RefCell<String>,

    /// When the span the next report will describe began.
    since: Cell<Option<Instant>>,
    /// Frames measured in it.
    frames: Cell<u64>,
    /// Frames in it that did not fit in their deadline.
    missed: Cell<u64>,
    /// The worst of those.
    worst: RefCell<Option<Slow>>,
    /// When the last report was made, if there has been one.
    reported: Cell<Option<Instant>>,
}

thread_local! {
    static COUNTERS: Counters = const {
        Counters {
            on: Cell::new(false),
            asked: Cell::new(false),
            live: Cell::new(false),
            started: Cell::new(None),
            mark: Cell::new(None),
            phase: Cell::new(Phase::Loose),
            spent: [const { Cell::new(0) }; Phase::COUNT],
            scenes: Cell::new(0),
            animating: Cell::new(0),
            rendered: Cell::new(0),
            rebound: Cell::new(0),
            drew: Cell::new(0),
            deadline: Cell::new(Duration::ZERO),
            monitor: RefCell::new(String::new()),
            since: Cell::new(None),
            frames: Cell::new(0),
            missed: Cell::new(0),
            worst: RefCell::new(None),
            reported: Cell::new(None),
        }
    };
}

/// A frame being measured. Ends with [`Frame::finish`].
///
/// Deliberately without a `Drop` that reports. Finishing needs to be told what
/// was on screen, and a guard that reported on the way out would either have to
/// go and fetch that itself — coupling this module to the whole of `state.rs` —
/// or report without it, which is the half of the line that makes the other
/// half mean anything.
#[derive(Debug)]
pub(crate) struct Frame {
    on: bool,
}

/// A phase, running for as long as this is held.
///
/// Restores the phase that was running before it on the way out, so the phases
/// stay exclusive however they nest.
#[derive(Debug)]
#[must_use = "the phase lasts only as long as this is held"]
pub(crate) struct Span(Option<Phase>);

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            COUNTERS.with(|counters| {
                if counters.live.get() {
                    counters.switch(Instant::now(), previous);
                }
            });
        }
    }
}

impl Counters {
    /// Close the phase that is running and open `next`, returning the one that
    /// was closed.
    ///
    /// The whole of the measurement, in six lines. `now` is passed in rather
    /// than sampled here so that a caller holding a timestamp already — the
    /// frame's start and end — does not take a second one a few nanoseconds
    /// later and charge the gap to nothing.
    fn switch(&self, now: Instant, next: Phase) -> Phase {
        let previous = self.phase.get();
        if let Some(mark) = self.mark.get() {
            let elapsed =
                u64::try_from(now.saturating_duration_since(mark).as_nanos()).unwrap_or(u64::MAX);
            let slot = &self.spent[previous.slot()];
            slot.set(slot.get().saturating_add(elapsed));
        }
        self.mark.set(Some(now));
        self.phase.set(next);
        previous
    }
}

/// Begin measuring a frame.
///
/// Called by whichever backend is drawing. Returns a handle that is `on` only
/// if the knob is; everything else in this module is a no-op until it is.
pub(crate) fn frame() -> Frame {
    COUNTERS.with(|counters| {
        if !counters.asked.get() {
            counters.asked.set(true);
            counters.on.set(crate::dev::pacing());
        }
        if !counters.on.get() {
            return Frame { on: false };
        }

        let now = Instant::now();
        for slot in &counters.spent {
            slot.set(0);
        }
        counters.scenes.set(0);
        counters.animating.set(0);
        counters.rendered.set(0);
        counters.rebound.set(0);
        counters.drew.set(0);
        counters.deadline.set(Duration::ZERO);
        counters.started.set(Some(now));
        counters.mark.set(Some(now));
        counters.phase.set(Phase::Loose);
        if counters.since.get().is_none() {
            counters.since.set(Some(now));
        }
        counters.live.set(true);
        Frame { on: true }
    })
}

/// Run `phase` until the returned guard is dropped.
///
/// Off, or outside a frame, this samples no clock and touches nothing.
pub(crate) fn span(phase: Phase) -> Span {
    COUNTERS.with(|counters| {
        if !counters.live.get() {
            return Span(None);
        }
        Span(Some(counters.switch(Instant::now(), phase)))
    })
}

/// A scene was asked whether it still has somewhere to go, and answered.
pub(crate) fn scene_asked(animating: bool) {
    COUNTERS.with(|counters| {
        if !counters.live.get() {
            return;
        }
        counters.scenes.set(counters.scenes.get().saturating_add(1));
        if animating {
            counters
                .animating
                .set(counters.animating.get().saturating_add(1));
        }
    });
}

/// A QML scene was actually re-rendered by Qt.
pub(crate) fn scene_rendered() {
    COUNTERS.with(|counters| {
        if counters.live.get() {
            counters
                .rendered
                .set(counters.rendered.get().saturating_add(1));
        }
    });
}

/// A QML scene was moved onto a newly allocated buffer.
///
/// The expensive case that is otherwise invisible: a GBM allocation, a dmabuf
/// export, an `eglCreateImageKHR`, a Qt render-target swap, a full Qt render
/// and an `import_dmabuf`. There is a per-size cache in front of it
/// (`qml::paint::Kept`), so a steady stream of these means the cache is being
/// thrashed and is worth seeing rather than assuming.
pub(crate) fn scene_rebound() {
    COUNTERS.with(|counters| {
        if counters.live.get() {
            counters
                .rebound
                .set(counters.rebound.get().saturating_add(1));
        }
    });
}

impl Frame {
    /// A frame that is not being measured.
    ///
    /// For a loop that decides whether to draw *after* it would have started
    /// measuring. The nested backend is one: it can skip a whole iteration when
    /// nothing has changed, and an iteration that drew nothing is not a slow
    /// frame — it is the compositor correctly asleep, and counting it would put
    /// the idle timeout inside the measurement.
    pub(crate) const fn off() -> Self {
        Self { on: false }
    }

    /// Whether anything is being measured, so a caller can skip work that only
    /// the report wants — the monitor's name is the whole of it.
    pub(crate) const fn on(&self) -> bool {
        self.on
    }

    /// This frame's deadline, and which monitor it belongs to.
    ///
    /// **The tightest interval among the monitors being driven, not the one
    /// being drawn.** Two monitors at 260 Hz and 75 Hz do not get two budgets,
    /// because they do not get two threads: the render pass is one pass on one
    /// event loop, so a pass that takes 10 ms to draw the 75 Hz screen has also
    /// held the 260 Hz screen off for 10 ms and cost it two frames. Judging
    /// that pass against 13.3 ms because of which output it happened to draw
    /// would call the thing that caused the stutter a success.
    ///
    /// So the deadline is per-output in the sense that matters — it is read
    /// from a real mode rather than assumed, and on a mixed-rate desk it is the
    /// fast monitor's — and `drew` in the report says how many screens the pass
    /// actually got to.
    pub(crate) fn deadline(&self, interval: Duration, monitor: &str) {
        if !self.on {
            return;
        }
        COUNTERS.with(|counters| {
            counters.deadline.set(interval);
            if let Ok(mut held) = counters.monitor.try_borrow_mut() {
                held.clear();
                held.push_str(monitor);
            }
        });
    }

    /// One more output was drawn this frame.
    pub(crate) fn drew(&self) {
        if !self.on {
            return;
        }
        COUNTERS.with(|counters| counters.drew.set(counters.drew.get().saturating_add(1)));
    }

    /// Stop measuring, and report if this frame missed and a report is due.
    ///
    /// `panes` is what was on screen — the count the reader needs to tell a
    /// slow frame with eight windows from a slow frame with one.
    pub(crate) fn finish(self, panes: usize) {
        if !self.on {
            return;
        }
        COUNTERS.with(|counters| {
            let now = Instant::now();
            counters.switch(now, Phase::Loose);
            counters.live.set(false);
            counters.mark.set(None);

            let Some(started) = counters.started.get() else {
                return;
            };
            let total = now.saturating_duration_since(started);
            counters.frames.set(counters.frames.get().saturating_add(1));

            // A deadline of zero is a backend that did not name one. Nothing
            // can be missed against it, and inventing a number would turn a
            // wiring mistake into a stream of confident nonsense.
            let deadline = counters.deadline.get();
            if deadline.is_zero() || total <= deadline {
                return;
            }
            counters.missed.set(counters.missed.get().saturating_add(1));

            let mut spent = [0_u64; Phase::COUNT];
            for (slot, cell) in spent.iter_mut().zip(counters.spent.iter()) {
                *slot = cell.get();
            }
            let slow = Slow {
                total,
                deadline,
                monitor: counters
                    .monitor
                    .try_borrow()
                    .map(|held| held.clone())
                    .unwrap_or_default(),
                spent,
                panes: u32::try_from(panes).unwrap_or(u32::MAX),
                drew: counters.drew.get(),
                scenes: counters.scenes.get(),
                animating: counters.animating.get(),
                rendered: counters.rendered.get(),
                rebound: counters.rebound.get(),
            };
            if let Ok(mut worst) = counters.worst.try_borrow_mut()
                && worst.as_ref().is_none_or(|held| slow.total > held.total)
            {
                *worst = Some(slow);
            }

            // The first miss after a quiet spell goes out at once; everything
            // after it waits for the limit. See `REPORT_EVERY`.
            let due = counters
                .reported
                .get()
                .is_none_or(|last| now.saturating_duration_since(last) >= REPORT_EVERY);
            if !due {
                return;
            }
            let span = counters
                .since
                .get()
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
            let worst = counters
                .worst
                .try_borrow_mut()
                .ok()
                .and_then(|mut held| held.take());
            let (frames, missed) = (counters.frames.get(), counters.missed.get());
            counters.reported.set(Some(now));
            counters.since.set(Some(now));
            counters.frames.set(0);
            counters.missed.set(0);
            if let Some(worst) = worst {
                report(&worst, frames, missed, span);
            }
        });
    }
}

/// Say what the worst frame of the span did.
///
/// Microseconds throughout, as integers. Not a pre-formatted string: these are
/// `tracing` fields so that one of them can be grepped, plotted or filtered
/// without parsing a sentence, which is what a frame-timing number is for.
fn report(worst: &Slow, frames: u64, missed: u64, span: Duration) {
    let phase = |which: Phase| worst.spent[which.slot()] / 1_000;
    // `tracing` records `u64`, not `u128`, and a `Duration` measures in the
    // latter. Saturating rather than truncating: a frame that somehow lasted
    // longer than half a million years should read as enormous, not as small.
    let micros = |duration: Duration| u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
    tracing::warn!(
        total_us = micros(worst.total),
        deadline_us = micros(worst.deadline),
        monitor = worst.monitor,
        missed,
        frames,
        span_ms = micros(span) / 1_000,
        tick_us = phase(Phase::Tick),
        prep_us = phase(Phase::Prep),
        census_us = phase(Phase::Census),
        qml_us = phase(Phase::Qml),
        elements_us = phase(Phase::Elements),
        gles_us = phase(Phase::Gles),
        commit_us = phase(Phase::Commit),
        settle_us = phase(Phase::Settle),
        loose_us = phase(Phase::Loose),
        panes = worst.panes,
        drew = worst.drew,
        scenes = worst.scenes,
        animating = worst.animating,
        rendered = worst.rendered,
        rebound = worst.rebound,
        "PACING"
    );
}

#[cfg(test)]
mod tests {
    use super::{Counters, Phase, REPORT_EVERY};
    use std::time::{Duration, Instant};

    /// The rate limiter, as arithmetic, with no clock and no compositor.
    ///
    /// Written against the same rule `Frame::finish` applies rather than
    /// against `Frame::finish` itself, because the thing worth pinning is the
    /// *policy* — one line per second whatever happens, and the first miss
    /// straight away — and the only way to drive the real one is to render
    /// frames, which needs a GPU and a Qt.
    struct Limiter {
        reported: Option<Duration>,
        lines: u32,
    }

    impl Limiter {
        const fn new() -> Self {
            Self {
                reported: None,
                lines: 0,
            }
        }

        /// One missed frame at `at`. Returns whether it produced a line.
        fn missed(&mut self, at: Duration) -> bool {
            let due = self
                .reported
                .is_none_or(|last| at.saturating_sub(last) >= REPORT_EVERY);
            if due {
                self.reported = Some(at);
                self.lines += 1;
            }
            due
        }
    }

    /// **A sustained stall costs one line a second, not one a frame.**
    ///
    /// The requirement this whole limiter exists for. Sixty seconds of a
    /// compositor missing every frame at 260 Hz is 15,600 slow frames; at a
    /// line each, into a log opened `O_DSYNC`, the diagnostic is the outage.
    #[test]
    fn a_sustained_stall_is_one_line_a_second() {
        let mut limiter = Limiter::new();
        // 260 Hz for sixty seconds, every frame over its deadline.
        for frame in 0..15_600_u32 {
            limiter.missed(Duration::from_nanos(u64::from(frame) * 3_846_153));
        }
        assert_eq!(
            limiter.lines, 60,
            "sixty seconds of solid stall should be sixty lines"
        );
    }

    /// And the first one goes out at once, rather than a second late.
    #[test]
    fn the_first_miss_reports_immediately() {
        let mut limiter = Limiter::new();
        assert!(limiter.missed(Duration::from_millis(17)));
    }

    /// A hiccup every few seconds is reported every time: the limit is a
    /// ceiling on the rate, not a sampling interval.
    #[test]
    fn an_occasional_miss_is_never_swallowed() {
        let mut limiter = Limiter::new();
        for second in 0..30_u64 {
            assert!(
                limiter.missed(Duration::from_secs(second * 5)),
                "a miss five seconds after the last report was dropped"
            );
        }
        assert_eq!(limiter.lines, 30);
    }

    /// **Phases are exclusive: a nested one does not also count in its
    /// parent.**
    ///
    /// The property the whole breakdown rests on. `qml` runs inside
    /// `elements`, and a reader who cannot tell 7 ms of Qt from 7 ms of
    /// compositor has a number and no culprit.
    #[test]
    fn a_nested_phase_is_taken_out_of_the_one_around_it() {
        let counters = counters();
        let start = Instant::now();
        // elements for 10 units, with qml for 4 of them in the middle.
        counters.switch(start, Phase::Elements);
        counters.switch(start + ms(3), Phase::Qml);
        counters.switch(start + ms(7), Phase::Elements);
        counters.switch(start + ms(10), Phase::Loose);

        assert_eq!(spent_ms(&counters, Phase::Qml), 4);
        assert_eq!(
            spent_ms(&counters, Phase::Elements),
            6,
            "Qt's four milliseconds were charged to the compositor as well"
        );
    }

    /// Time no phase claimed is reported rather than dropped, so a breakdown
    /// that stops adding up says so instead of quietly under-reporting.
    #[test]
    fn unclaimed_time_lands_in_loose() {
        let counters = counters();
        let start = Instant::now();
        counters.switch(start, Phase::Loose);
        counters.switch(start + ms(5), Phase::Gles);
        counters.switch(start + ms(6), Phase::Loose);
        assert_eq!(spent_ms(&counters, Phase::Loose), 5);
        assert_eq!(spent_ms(&counters, Phase::Gles), 1);
    }

    /// Every phase has a counter of its own, and there are as many counters as
    /// phases.
    ///
    /// `Phase::slot` is `self as usize` over a plain enum, so this is only ever
    /// wrong in one way — a phase added without widening [`Phase::COUNT`],
    /// which would index past the array and be saturated into whichever slot
    /// the arithmetic landed on. That is a silent wrong number in a diagnostic,
    /// which is worse than no diagnostic.
    #[test]
    fn every_phase_has_a_counter_of_its_own() {
        let all = [
            Phase::Tick,
            Phase::Prep,
            Phase::Census,
            Phase::Qml,
            Phase::Elements,
            Phase::Gles,
            Phase::Commit,
            Phase::Settle,
            Phase::Loose,
        ];
        assert_eq!(all.len(), Phase::COUNT, "a phase was added and not listed");
        let mut slots: Vec<usize> = all.iter().map(|phase| phase.slot()).collect();
        assert!(
            slots.iter().all(|slot| *slot < Phase::COUNT),
            "a phase indexes past its counter array"
        );
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), Phase::COUNT, "two phases share a counter");
    }

    fn ms(count: u64) -> Duration {
        Duration::from_millis(count)
    }

    fn spent_ms(counters: &Counters, phase: Phase) -> u64 {
        counters.spent[phase.slot()].get() / 1_000_000
    }

    /// A counter set outside the thread-local, so a test can drive `switch`
    /// with timestamps of its own rather than with the clock.
    fn counters() -> Counters {
        Counters {
            on: std::cell::Cell::new(true),
            asked: std::cell::Cell::new(true),
            live: std::cell::Cell::new(true),
            started: std::cell::Cell::new(None),
            mark: std::cell::Cell::new(None),
            phase: std::cell::Cell::new(Phase::Loose),
            spent: [const { std::cell::Cell::new(0) }; Phase::COUNT],
            scenes: std::cell::Cell::new(0),
            animating: std::cell::Cell::new(0),
            rendered: std::cell::Cell::new(0),
            rebound: std::cell::Cell::new(0),
            drew: std::cell::Cell::new(0),
            deadline: std::cell::Cell::new(Duration::ZERO),
            monitor: std::cell::RefCell::new(String::new()),
            since: std::cell::Cell::new(None),
            frames: std::cell::Cell::new(0),
            missed: std::cell::Cell::new(0),
            worst: std::cell::RefCell::new(None),
            reported: std::cell::Cell::new(None),
        }
    }
}
