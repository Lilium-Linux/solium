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
//! * **It has to be nearly free when off.** A hardware session runs at the
//!   monitor's refresh — 260 Hz on the desk this was written for — so anything
//!   paid per frame is paid 260 times a second. Off, a pass reads the clock
//!   twice (a vDSO `clock_gettime`, tens of nanoseconds) and counts itself and
//!   its miss, because a miss with the knob off is still a miss
//!   (`tests::a_miss_is_counted_with_the_knob_off`); on the hardware a frame
//!   queued reads it once more, and its flip counts how late it landed
//!   (`tests::a_late_flip_is_counted_with_the_knob_off`). Nothing allocates
//!   and nothing formats. It is deliberately not a compile-time feature: a
//!   diagnostic that is not in the shipped binary is not there on the day the
//!   shipped binary is slow, which is the only day it is wanted.
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
//!   [`REPORT_EVERY`]. `SOLIUM_TRACE`, which wants every pass, formats one
//!   record a pass into a buffer of its own and writes it once a second,
//!   never `O_DSYNC`: [`Trace`],
//!   `tests::the_trace_is_buffered_and_flushed_once_a_second`.
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
    io::Write as _,
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
/// plus how many frames missed out of how many were drawn and over what span,
/// and how many vblanks a flip missed in it (`late`, which makes a report due
/// on its own: `tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`).
/// So a single hiccup reads `missed=1 frames=3` and a sustained stall reads
/// `missed=247 frames=259`, and the two are never confusable.
///
/// The first miss or late flip after a quiet spell reports immediately rather
/// than waiting out a window — a diagnostic whose first evidence arrives a
/// second late is hard to trust, and it is the case somebody is watching the
/// log live for. A late flip seen as the loop goes idle reports then, with no
/// pass after it: `tests::a_late_flip_before_idle_is_reported_at_idle`.
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
    /// Qt's event queue drained. Once per frame, never once per output. Also
    /// the models' rows, applied just before it (`Solium::publish_models`),
    /// because applying a row runs every binding and handler on it:
    /// `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.
    Tick,
    /// The rest of `render::prepare`, plus `screencopy::settle`: one
    /// offscreen render pass for every window whose transform is not a
    /// rectangle.
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

/// A QML scene, as these counters name it: interned once when the scene is
/// built, never per pass. `tests::qml_time_is_charged_to_the_scene_that_spent_it`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SceneId(u32);

impl SceneId {
    /// No scene, as every scene is with the knob off:
    /// `tests::off_charges_nothing`.
    pub(crate) const NONE: Self = Self(u32::MAX);
}

/// Scenes one pass charges separately; the rest share "other".
/// `tests::more_scenes_than_slots_land_in_other`.
const SCENE_SLOTS: usize = 8;

/// One frame's measurements, kept for as long as it is the worst one seen.
///
/// A snapshot rather than a borrow of the live state, because the live state is
/// about to be reused by the next frame and this one may not be reported for
/// another second.
#[derive(Clone, Debug)]
struct Slow {
    pass: u64,
    total: Duration,
    deadline: Duration,
    monitor: String,
    spent: [u64; Phase::COUNT],
    panes: u32,
    drew: u32,
    scenes: u32,
    animating: u32,
    rendered: u32,
    built: u32,
    rebound: u32,
    gpu: Option<crate::gputime::Gpu>,
    captures: u32,
    /// The GPU's clocks as it ended, so a line made long after its pass
    /// shows that pass's clocks:
    /// `tests::a_line_made_long_after_its_pass_carries_that_passes_clocks`.
    clocks: Option<crate::clocks::Clocks>,
    /// The pass's three costliest scenes, as `Counters::top` says them.
    /// `tests::a_line_names_its_slowest_passes_costliest_scenes`.
    qml_top: String,
}

/// Everything this module holds, for one thread.
///
/// `Cell` throughout on the hot path rather than a `RefCell`: the phase switch
/// runs tens of times a frame and `RefCell` can panic, which the workspace
/// denies for good reason — a compositor crash takes the session with it, and a
/// diagnostic that can end a session is strictly worse than no diagnostic. The
/// `RefCell`s hold strings, snapshots and the trace, are touched a few times a
/// pass at most, and are only ever entered with `try_borrow` or
/// `try_borrow_mut`.
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
    /// How many QML scenes were *built* — a file compiled and instantiated
    /// into an object tree. Rare, very expensive, and not only a startup
    /// event: a window opening builds its decoration.
    built: Cell<u32>,
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
    /// Vblanks a flip missed in it.
    /// `tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`.
    late: Cell<u64>,
    /// The slowest pass in the span, missed or not:
    /// `tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`.
    worst: RefCell<Option<Slow>>,
    /// When the last report was made, if there has been one.
    reported: Cell<Option<Instant>>,
    /// Every pass since the session began, measured or not.
    /// `tests::a_miss_is_counted_with_the_knob_off`.
    all_passes: Cell<u64>,
    /// How many of them overran the tightest monitor's interval.
    all_missed: Cell<u64>,
    /// Vblanks a flip missed since the session began, measured or not.
    /// `tests::a_late_flip_is_counted_with_the_knob_off`.
    all_late: Cell<u64>,
    /// A report that is due and waits for its pass's GPU time.
    /// `tests::a_report_waits_for_its_passes_gpu_time`.
    parked: RefCell<Option<Line>>,
    /// The pass at which it was parked.
    parked_at: Cell<u64>,
    /// Captures drawn in this pass. `tests::a_capture_is_counted_only_inside_a_measured_pass`.
    captures: Cell<u32>,
    /// This pass's GPU time, when it came before the pass ended, as a timer
    /// with no extension answers.
    /// `tests::a_gpu_time_in_before_its_pass_ends_goes_out_with_it`.
    early: Cell<Option<crate::gputime::Gpu>>,
    /// The GPU's clocks as this pass ended, from the sampler:
    /// `tests::a_pass_takes_its_clocks_from_the_sampler`.
    clocks: Cell<Option<crate::clocks::Clocks>>,
    /// Each scene's label, by id; `None` for an id given back.
    /// `tests::a_forgotten_scene_gives_its_id_back`.
    labels: RefCell<Vec<Option<String>>>,
    /// Ids given back, to be given out again.
    /// `tests::a_forgotten_scene_gives_its_id_back`.
    free_ids: RefCell<Vec<u32>>,
    /// This pass's Qt time per scene: `(id + 1, ns)`, 0 for an empty slot.
    /// `tests::qml_time_is_charged_to_the_scene_that_spent_it`,
    /// `tests::each_pass_charges_its_scenes_afresh`.
    scene_spent: [Cell<(u32, u64)>; SCENE_SLOTS],
    /// This pass's Qt time for scenes past the slots.
    /// `tests::more_scenes_than_slots_land_in_other`.
    scene_other: Cell<u64>,
    /// `SOLIUM_TRACE`'s file, when it is set and could be opened.
    /// `tests::a_counted_pass_is_traced_with_its_gpu_time`.
    trace: RefCell<Option<Trace>>,
    /// CLOCK_MONOTONIC as this pass began, with a trace open.
    /// `tests::a_traced_pass_and_its_flip_through_the_backends_calls`.
    t_ns: Cell<u64>,
    /// The windows on screen as this pass ended, for its record.
    /// `tests::every_pass_record_carries_the_documented_fields`.
    panes_seen: Cell<u32>,
}

thread_local! {
    static COUNTERS: Counters = const {
        Counters {
            on: Cell::new(false),
            asked: Cell::new(false),
            live: Cell::new(false),
            mark: Cell::new(None),
            phase: Cell::new(Phase::Loose),
            spent: [const { Cell::new(0) }; Phase::COUNT],
            scenes: Cell::new(0),
            animating: Cell::new(0),
            rendered: Cell::new(0),
            built: Cell::new(0),
            rebound: Cell::new(0),
            drew: Cell::new(0),
            deadline: Cell::new(Duration::ZERO),
            monitor: RefCell::new(String::new()),
            since: Cell::new(None),
            frames: Cell::new(0),
            missed: Cell::new(0),
            late: Cell::new(0),
            worst: RefCell::new(None),
            reported: Cell::new(None),
            all_passes: Cell::new(0),
            all_missed: Cell::new(0),
            all_late: Cell::new(0),
            parked: RefCell::new(None),
            parked_at: Cell::new(0),
            captures: Cell::new(0),
            early: Cell::new(None),
            clocks: Cell::new(None),
            labels: RefCell::new(Vec::new()),
            free_ids: RefCell::new(Vec::new()),
            scene_spent: [const { Cell::new((0, 0)) }; SCENE_SLOTS],
            scene_other: Cell::new(0),
            trace: RefCell::new(None),
            t_ns: Cell::new(0),
            panes_seen: Cell::new(0),
        }
    };
}

/// A pass being drawn. Ends with [`Frame::finish`].
///
/// Deliberately without a `Drop` that reports. Finishing needs to be told what
/// was on screen, and a guard that reported on the way out would either have to
/// go and fetch that itself — coupling this module to the whole of `state.rs` —
/// or report without it, which is the half of the line that makes the other
/// half mean anything.
#[derive(Debug)]
pub(crate) struct Frame {
    /// Whether the phases are measured: `SOLIUM_PACING`.
    on: bool,
    /// When the pass began; `None` for a loop iteration that draws nothing.
    started: Option<Instant>,
    /// Which pass this is, counted from 1 for the life of the process.
    pass: u64,
}

/// A phase, running for as long as this is held.
///
/// Restores the phase that was running before it on the way out, so the phases
/// stay exclusive however they nest. One made by [`qml`] also charges the time
/// it was held to its scene:
/// `tests::a_scenes_span_charges_it_and_the_phase_alike`.
#[derive(Debug)]
#[must_use = "the phase lasts only as long as this is held"]
pub(crate) struct Span {
    /// The phase to go back to; `None` when nothing is being measured.
    previous: Option<Phase>,
    /// The scene the time is charged to, and when it began.
    scene: Option<(SceneId, Instant)>,
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            COUNTERS.with(|counters| {
                if counters.live.get() {
                    let now = Instant::now();
                    counters.switch(now, previous);
                    if let Some((id, since)) = self.scene {
                        let nanos = u64::try_from(now.saturating_duration_since(since).as_nanos())
                            .unwrap_or(u64::MAX);
                        counters.charge(id, nanos);
                    }
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

    /// Count one pass that began at `started` and ended at `now`, and say
    /// whether a report is due.
    ///
    /// No clock is read here, so the rules are driven with made-up times:
    /// `tests::a_sustained_stall_is_one_line_a_second`,
    /// `tests::the_first_miss_reports_immediately`,
    /// `tests::an_occasional_miss_is_never_swallowed`,
    /// `tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`. A deadline of zero is a
    /// backend that did not name one: nothing can be missed against it, and
    /// inventing a number would turn a wiring mistake into a stream of
    /// confident nonsense.
    fn finish_at(
        &self,
        started: Instant,
        now: Instant,
        on: bool,
        panes: usize,
        pass: u64,
    ) -> Option<Line> {
        self.panes_seen
            .set(u32::try_from(panes).unwrap_or(u32::MAX));
        let total = now.saturating_duration_since(started);
        let early = self.early.take();
        let deadline = self.deadline.get();
        let missed = !deadline.is_zero() && total > deadline;
        self.all_passes.set(self.all_passes.get().saturating_add(1));
        if missed {
            self.all_missed.set(self.all_missed.get().saturating_add(1));
        }
        if !on {
            return None;
        }
        // The pass's record waits for its GPU time, unless that came before
        // the pass ended: `tests::a_counted_pass_is_traced_with_its_gpu_time`.
        if let Ok(mut trace) = self.trace.try_borrow_mut()
            && let Some(trace) = trace.as_mut()
        {
            trace.pass(pass, self.pass_record(pass, total, missed));
            if early.is_some() {
                trace.resolved(pass, early);
            }
            trace.tick(now);
        }
        self.frames.set(self.frames.get().saturating_add(1));
        if missed {
            self.missed.set(self.missed.get().saturating_add(1));
        }

        // The span's slowest pass, whether or not it missed, so a span with
        // only late flips still has a pass to show; snapshot only when it is
        // the slowest, so a pass that is not allocates nothing.
        // `tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`.
        if let Ok(mut worst) = self.worst.try_borrow_mut()
            && worst.as_ref().is_none_or(|held| total > held.total)
        {
            let mut spent = [0_u64; Phase::COUNT];
            for (slot, cell) in spent.iter_mut().zip(self.spent.iter()) {
                *slot = cell.get();
            }
            *worst = Some(Slow {
                pass,
                total,
                deadline,
                monitor: self
                    .monitor
                    .try_borrow()
                    .map(|held| held.clone())
                    .unwrap_or_default(),
                spent,
                panes: u32::try_from(panes).unwrap_or(u32::MAX),
                drew: self.drew.get(),
                scenes: self.scenes.get(),
                animating: self.animating.get(),
                rendered: self.rendered.get(),
                built: self.built.get(),
                rebound: self.rebound.get(),
                gpu: early,
                captures: self.captures.get(),
                clocks: self.clocks.get(),
                qml_top: self.top(),
            });
        }
        if !missed && self.late.get() == 0 {
            return None;
        }
        self.report(now)
    }

    /// The span's report, if one is due at `now`: its slowest pass, and what
    /// the span came to. The first miss or late flip after a quiet spell goes
    /// out at once; everything after it waits for the limit. See
    /// `REPORT_EVERY`. A span with no pass in it has none to show, and keeps
    /// what it counted for the next:
    /// `tests::a_late_flip_before_idle_is_reported_at_idle`.
    fn report(&self, now: Instant) -> Option<Line> {
        let due = self
            .reported
            .get()
            .is_none_or(|last| now.saturating_duration_since(last) >= REPORT_EVERY);
        if !due {
            return None;
        }
        let worst = self
            .worst
            .try_borrow_mut()
            .ok()
            .and_then(|mut held| held.take())?;
        let span = self
            .since
            .get()
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        let (frames, missed, late) = (self.frames.get(), self.missed.get(), self.late.get());
        self.reported.set(Some(now));
        self.since.set(Some(now));
        self.frames.set(0);
        self.missed.set(0);
        self.late.set(0);
        Some(Line::of(&worst, frames, missed, late, span))
    }

    /// The loop went idle: the trace's buffer is written out, and a record
    /// still waiting for its GPU time keeps waiting, because nested that time
    /// is read only as the next pass begins
    /// (`tests::a_record_waiting_as_the_loop_goes_idle_keeps_waiting_for_its_gpu_time`);
    /// then whatever is parked (`tests::idle_flushes_a_parked_report`), or
    /// else a report the span's late flips made due, since a flip seen just
    /// before the loop went idle has no pass after it to report it
    /// (`tests::a_late_flip_before_idle_is_reported_at_idle`). `now` is
    /// asked for only then, so with the knob off this reads no clock.
    fn idle(&self, now: impl FnOnce() -> Instant) -> Option<Line> {
        if let Ok(mut trace) = self.trace.try_borrow_mut()
            && let Some(trace) = trace.as_mut()
        {
            trace.flush();
        }
        if let Some(line) = self.flush() {
            return Some(line);
        }
        if self.late.get() == 0 {
            return None;
        }
        self.report(now())
    }

    /// The session ended: the trace's waiting records are written, as late,
    /// and then it goes idle for the last time
    /// (`tests::a_counted_pass_is_traced_with_its_gpu_time`).
    fn end(&self, now: impl FnOnce() -> Instant) -> Option<Line> {
        if let Ok(mut trace) = self.trace.try_borrow_mut()
            && let Some(trace) = trace.as_mut()
        {
            trace.close();
        }
        self.idle(now)
    }

    /// The session's totals. `tests::a_miss_is_counted_with_the_knob_off`.
    fn totals(&self) -> Totals {
        Totals {
            passes: self.all_passes.get(),
            missed: self.all_missed.get(),
            late: self.all_late.get(),
        }
    }

    /// Whether pacing is on, decided from the environment once: either
    /// knob, `SOLIUM_TRACE` opening its file too
    /// (`tests::the_trace_turns_pacing_on`).
    fn decide(&self) -> bool {
        if !self.asked.get() {
            self.asked.set(true);
            let path = crate::dev::trace_path();
            self.on.set(knob(crate::dev::pacing(), path.is_some()));
            if let Some(path) = path
                && let Ok(mut trace) = self.trace.try_borrow_mut()
            {
                *trace = Trace::open(&path, Instant::now());
            }
        }
        self.on.get()
    }

    /// One capture was drawn. `tests::a_capture_is_counted_only_inside_a_measured_pass`.
    fn capture(&self) {
        if self.live.get() {
            self.captures.set(self.captures.get().saturating_add(1));
        }
    }

    /// Hold a due report until its pass's GPU time is in.
    fn park(&self, line: Line, pass: u64) {
        if let Ok(mut held) = self.parked.try_borrow_mut() {
            *held = Some(line);
            self.parked_at.set(pass);
        }
    }

    /// Whatever is parked, now. `tests::idle_flushes_a_parked_report`.
    fn flush(&self) -> Option<Line> {
        self.parked
            .try_borrow_mut()
            .ok()
            .and_then(|mut held| held.take())
    }

    /// The parked report, once [`PARK_PASSES`] passes have gone by without its
    /// GPU time. `tests::a_report_goes_out_without_gpu_time_after_eight_passes`.
    fn waited(&self, pass: u64) -> Option<Line> {
        let parked = self.parked.try_borrow().is_ok_and(|held| held.is_some());
        (parked && pass.saturating_sub(self.parked_at.get()) >= PARK_PASSES)
            .then(|| self.flush())
            .flatten()
    }

    /// A pass's GPU time is in: keep it with the worst pass if it is that one,
    /// or with the pass being measured if it is that one, and send the parked
    /// report if it was waiting for it.
    /// `tests::a_gpu_time_that_came_before_its_report_goes_out_with_it`.
    /// The trace's record of that pass goes with it:
    /// `tests::a_counted_pass_is_traced_with_its_gpu_time`.
    fn gpu_resolved(&self, pass: u64, gpu: crate::gputime::Gpu) -> Option<Line> {
        if let Ok(mut trace) = self.trace.try_borrow_mut()
            && let Some(trace) = trace.as_mut()
        {
            trace.resolved(pass, Some(gpu));
        }
        if let Ok(mut worst) = self.worst.try_borrow_mut()
            && let Some(held) = worst.as_mut()
            && held.pass == pass
        {
            held.gpu = Some(gpu);
        }
        // `frame` numbers the pass being measured after those counted:
        // `tests::a_gpu_time_in_before_its_pass_ends_goes_out_with_it`.
        if self.live.get() && pass == self.all_passes.get().saturating_add(1) {
            self.early.set(Some(gpu));
            return None;
        }
        let waiting = self
            .parked
            .try_borrow()
            .is_ok_and(|held| held.as_ref().is_some_and(|line| line.pass == pass));
        if !waiting {
            return None;
        }
        let mut line = self.flush()?;
        line.gpu = Some(gpu);
        Some(line)
    }

    /// A flip landed `late` vblanks late: into the session's totals always
    /// (`tests::a_late_flip_is_counted_with_the_knob_off`), and into the
    /// span's report with the knob on
    /// (`tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`).
    fn flipped(&self, late: u32) {
        let late = u64::from(late);
        self.all_late.set(self.all_late.get().saturating_add(late));
        if self.on.get() {
            self.late.set(self.late.get().saturating_add(late));
        }
    }

    /// Name a scene, once, when it is built: an id given back before, or a
    /// new one. `tests::a_forgotten_scene_gives_its_id_back`,
    /// `tests::off_charges_nothing`.
    fn intern(&self, label: &str) -> SceneId {
        if !self.on.get() {
            return SceneId::NONE;
        }
        let (Ok(mut labels), Ok(mut free)) =
            (self.labels.try_borrow_mut(), self.free_ids.try_borrow_mut())
        else {
            return SceneId::NONE;
        };
        let id = match free.pop() {
            Some(id) => id,
            None => {
                labels.push(None);
                u32::try_from(labels.len().saturating_sub(1)).unwrap_or(u32::MAX)
            }
        };
        if let Some(slot) = labels.get_mut(usize::try_from(id).unwrap_or(usize::MAX)) {
            *slot = Some(label.to_owned());
        }
        SceneId(id)
    }

    /// A scene was freed: its id is given back.
    /// `tests::a_forgotten_scene_gives_its_id_back`.
    fn forget(&self, id: SceneId) {
        if id == SceneId::NONE {
            return;
        }
        if let (Ok(mut labels), Ok(mut free)) =
            (self.labels.try_borrow_mut(), self.free_ids.try_borrow_mut())
            && let Some(slot) = labels.get_mut(usize::try_from(id.0).unwrap_or(usize::MAX))
            && slot.take().is_some()
        {
            free.push(id.0);
        }
    }

    /// Charge `nanos` of Qt's time to `id`, in this pass. No allocation.
    /// `tests::qml_time_is_charged_to_the_scene_that_spent_it`,
    /// `tests::more_scenes_than_slots_land_in_other`.
    fn charge(&self, id: SceneId, nanos: u64) {
        if id == SceneId::NONE {
            return;
        }
        let key = id.0.saturating_add(1);
        let slot = self
            .scene_spent
            .iter()
            .find(|slot| slot.get().0 == key)
            .or_else(|| self.scene_spent.iter().find(|slot| slot.get().0 == 0));
        match slot {
            Some(slot) => slot.set((key, slot.get().1.saturating_add(nanos))),
            None => self
                .scene_other
                .set(self.scene_other.get().saturating_add(nanos)),
        }
    }

    /// This pass's time for one scene. For the tests.
    #[cfg(test)]
    fn spent_by(&self, id: SceneId) -> u64 {
        let key = id.0.saturating_add(1);
        self.scene_spent
            .iter()
            .find(|slot| slot.get().0 == key)
            .map_or(0, |slot| slot.get().1)
    }

    /// The three costliest scenes of this pass, as `label=us`, costliest
    /// first. Formatted only for a pass that is its span's slowest so far,
    /// into its snapshot: `tests::the_report_names_the_three_costliest_scenes`,
    /// `tests::a_line_names_its_slowest_passes_costliest_scenes`.
    fn top(&self) -> String {
        let mut spent: Vec<(u32, u64)> = self
            .scene_spent
            .iter()
            .map(Cell::get)
            .filter(|(key, _)| *key != 0)
            .collect();
        spent.sort_by_key(|&(_, nanos)| std::cmp::Reverse(nanos));
        let Ok(labels) = self.labels.try_borrow() else {
            return String::new();
        };
        spent
            .iter()
            .take(3)
            .filter_map(|(key, nanos)| {
                let label = labels
                    .get(usize::try_from(key.checked_sub(1)?).ok()?)?
                    .as_ref()?;
                Some(format!("{label}={}", nanos / 1_000))
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    /// This pass's record, without its GPU half.
    /// `tests::every_pass_record_carries_the_documented_fields`.
    fn pass_record(&self, pass: u64, total: Duration, missed: bool) -> String {
        let micros = |duration: Duration| u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        let phase = |which: Phase| self.spent[which.slot()].get() / 1_000;
        let monitor = self
            .monitor
            .try_borrow()
            .map(|held| held.clone())
            .unwrap_or_default();
        let mut qml = String::from("{");
        if let Ok(labels) = self.labels.try_borrow() {
            // Scenes built from one file share a label, every window's frame
            // in a style among them, and a JSON object holds a name once, so
            // theirs are summed: `tests::scenes_that_share_a_label_are_summed_in_a_record`.
            let mut spent: Vec<(&str, u64)> = Vec::with_capacity(SCENE_SLOTS);
            for (key, nanos) in self
                .scene_spent
                .iter()
                .map(Cell::get)
                .filter(|(key, _)| *key != 0)
            {
                let label = usize::try_from(key.saturating_sub(1))
                    .ok()
                    .and_then(|id| labels.get(id))
                    .and_then(Option::as_deref);
                if let Some(label) = label {
                    match spent.iter_mut().find(|(seen, _)| *seen == label) {
                        Some((_, sum)) => *sum = sum.saturating_add(nanos),
                        None => spent.push((label, nanos)),
                    }
                }
            }
            for (label, nanos) in spent {
                if qml.len() > 1 {
                    qml.push(',');
                }
                qml.push_str(&format!(
                    "{}:{}",
                    crate::scripted::json_string(label),
                    nanos / 1_000
                ));
            }
        }
        qml.push('}');
        let clocks = self.clocks.get().unwrap_or_default();
        format!(
            concat!(
                r#"{{"pass":{},"t_ns":{},"total_us":{},"deadline_us":{},"monitor":{},"missed":{},"#,
                r#""tick_us":{},"prep_us":{},"census_us":{},"qml_us":{},"elements_us":{},"gles_us":{},"#,
                r#""commit_us":{},"settle_us":{},"loose_us":{},"captures":{},"panes":{},"drew":{},"#,
                r#""scenes":{},"animating":{},"rendered":{},"built":{},"rebound":{},"qml":{},"#,
                r#""clocks":"{}","gpu_mhz":{},"mem_mhz":{},"pstate":{}"#
            ),
            pass,
            self.t_ns.get(),
            micros(total),
            micros(self.deadline.get()),
            crate::scripted::json_string(&monitor),
            missed,
            phase(Phase::Tick),
            phase(Phase::Prep),
            phase(Phase::Census),
            phase(Phase::Qml),
            phase(Phase::Elements),
            phase(Phase::Gles),
            phase(Phase::Commit),
            phase(Phase::Settle),
            phase(Phase::Loose),
            self.captures.get(),
            self.panes_seen.get(),
            self.drew.get(),
            self.scenes.get(),
            self.animating.get(),
            self.rendered.get(),
            self.built.get(),
            self.rebound.get(),
            qml,
            crate::clocks::source().name(),
            clocks.gpu_mhz,
            clocks.mem_mhz,
            clocks.pstate.map_or(-1, i32::from)
        )
    }
}

/// Passes a due report waits for its GPU time: read at least three passes
/// late (`gputime`), and a little more for a busy queue.
const PARK_PASSES: u64 = 8;

/// Begin a pass.
///
/// Always reads the clock once, because every pass is counted: a miss with the
/// knob off is still a miss (`tests::a_miss_is_counted_with_the_knob_off`,
/// and through `frame`, `deadline` and `finish` as the backends call them,
/// `tests::a_pass_with_the_knob_off_is_counted_from_frame_to_finish`).
/// The phases, the snapshot and the line need the knob.
pub(crate) fn frame() -> Frame {
    COUNTERS.with(|counters| {
        let on = counters.decide();
        let now = Instant::now();
        let pass = counters.all_passes.get().saturating_add(1);
        counters.deadline.set(Duration::ZERO);
        if !on {
            return Frame {
                on: false,
                started: Some(now),
                pass,
            };
        }

        for slot in &counters.spent {
            slot.set(0);
        }
        counters.scenes.set(0);
        counters.animating.set(0);
        counters.rendered.set(0);
        counters.built.set(0);
        counters.rebound.set(0);
        counters.drew.set(0);
        counters.captures.set(0);
        for slot in &counters.scene_spent {
            slot.set((0, 0));
        }
        counters.scene_other.set(0);
        // `tests::a_traced_pass_and_its_flip_through_the_backends_calls`.
        if counters
            .trace
            .try_borrow()
            .is_ok_and(|trace| trace.is_some())
        {
            counters
                .t_ns
                .set(u64::try_from(monotonic_now().as_nanos()).unwrap_or(u64::MAX));
        }
        counters.mark.set(Some(now));
        counters.phase.set(Phase::Loose);
        if counters.since.get().is_none() {
            counters.since.set(Some(now));
        }
        counters.live.set(true);
        // A report that has waited long enough for its GPU time goes without it.
        if let Some(line) = counters.waited(pass) {
            emit(&line);
        }
        Frame {
            on: true,
            started: Some(now),
            pass,
        }
    })
}

/// Run `phase` until the returned guard is dropped.
///
/// Off, or outside a frame, this samples no clock and touches nothing.
pub(crate) fn span(phase: Phase) -> Span {
    COUNTERS.with(|counters| {
        if !counters.live.get() {
            return Span {
                previous: None,
                scene: None,
            };
        }
        Span {
            previous: Some(counters.switch(Instant::now(), phase)),
            scene: None,
        }
    })
}

/// A scene's label: its file and the folder it is in, as `rounded/Ring`.
/// `tests::a_scene_is_labelled_by_its_folder_and_file`.
pub(crate) fn label_of(path: &std::path::Path) -> String {
    let file = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    match path.parent().and_then(std::path::Path::file_name) {
        Some(folder) => format!("{}/{file}", folder.to_string_lossy()),
        None => file,
    }
}

/// Intern a scene's label, once, when it is built.
/// `qml::hosting_tests::a_scenes_build_and_render_are_charged_to_it`.
pub(crate) fn scene_id(label: &str) -> SceneId {
    COUNTERS.with(|counters| {
        counters.decide();
        counters.intern(label)
    })
}

/// A scene was freed: its id is given back.
/// `qml::hosted::tests::a_hosted_scene_is_named_by_its_file_and_monitor_and_gives_it_back`.
pub(crate) fn forget_scene(id: SceneId) {
    COUNTERS.with(|counters| counters.forget(id));
}

/// Qt's phase, charged to `id` as well, for as long as the guard is held.
/// `tests::a_scenes_span_charges_it_and_the_phase_alike`.
pub(crate) fn qml(id: SceneId) -> Span {
    COUNTERS.with(|counters| {
        if !counters.live.get() {
            return Span {
                previous: None,
                scene: None,
            };
        }
        let now = Instant::now();
        Span {
            previous: Some(counters.switch(now, Phase::Qml)),
            scene: Some((id, now)),
        }
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

/// A QML scene was built: a file compiled and instantiated into an object tree.
///
/// The single most expensive thing Qt is asked to do inside a frame, and not a
/// startup-only event — a window opening builds its decoration's scene, and a
/// script can declare a surface whenever it likes. Measured nested at 283 ms
/// for the first scene of a session, which is 78 frames of a 260 Hz monitor
/// spent inside one call.
pub(crate) fn scene_built() {
    COUNTERS.with(|counters| {
        if counters.live.get() {
            counters.built.set(counters.built.get().saturating_add(1));
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
    /// A loop iteration that draws nothing, and is therefore not a pass.
    ///
    /// For a loop that decides whether to draw *after* it would have started
    /// measuring. The nested backend is one: an iteration that drew nothing is
    /// the compositor correctly asleep, and counting it would put the idle
    /// timeout inside the measurement.
    pub(crate) const fn off() -> Self {
        Self {
            on: false,
            started: None,
            pass: 0,
        }
    }

    /// This pass's deadline, and which monitor it belongs to.
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
    ///
    /// The interval is taken on every pass, because every pass is judged; the
    /// name only with the knob on, because `Output::name` allocates and only
    /// the report reads it. `tests::the_deadline_takes_no_name_with_the_knob_off`.
    pub(crate) fn deadline(&self, interval: Duration, monitor: impl FnOnce() -> String) {
        if self.started.is_none() {
            return;
        }
        COUNTERS.with(|counters| {
            counters.deadline.set(interval);
            if self.on
                && let Ok(mut held) = counters.monitor.try_borrow_mut()
            {
                *held = monitor();
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

    /// Stop measuring, count the pass, and report if it missed or a flip was
    /// late since the last report (`tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`)
    /// and a report is due: at once if its GPU time is in, and otherwise
    /// parked until it is (`tests::a_report_waits_for_its_passes_gpu_time`).
    ///
    /// `panes` is what was on screen — the count the reader needs to tell a
    /// slow frame with eight windows from a slow frame with one.
    pub(crate) fn finish(self, panes: usize) {
        let Some(started) = self.started else {
            return;
        };
        COUNTERS.with(|counters| {
            let now = Instant::now();
            if self.on {
                counters.switch(now, Phase::Loose);
                counters.live.set(false);
                counters.mark.set(None);
                counters.clocks.set(crate::clocks::latest());
            }
            if let Some(line) = counters.finish_at(started, now, self.on, panes, self.pass) {
                // One report at a time: an older one still waiting goes as it is.
                if let Some(older) = counters.flush() {
                    emit(&older);
                }
                if line.gpu.is_some() {
                    emit(&line);
                } else {
                    counters.park(line, self.pass);
                }
            }
        });
    }

    /// This pass's number, counted from 1, the one its GPU time comes back
    /// under: `tests::a_pass_with_the_knob_off_is_counted_from_frame_to_finish`.
    pub(crate) const fn serial(&self) -> u64 {
        self.pass
    }
}

/// One page flip, as the kernel reported it: its vblank sequence and its
/// CLOCK_MONOTONIC time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Flip {
    pub(crate) seq: u32,
    pub(crate) at: Duration,
}

/// A frame queued on one screen, waiting for its flip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Queued {
    /// CLOCK_MONOTONIC when `queue_frame` returned.
    pub(crate) at: Duration,
    /// The vblank whose handler drew it, when one did: a frame chained to the
    /// last flip should land on the next.
    pub(crate) after: Option<Flip>,
    /// The pass that drew it, which its flip's record names.
    /// `tests::a_flip_record_is_written_at_the_vblank`.
    pub(crate) pass: u64,
}

/// Slack for the kernel's timestamp against ours, beyond the screen's
/// vertical blank: a fifth of a millisecond. The price is that a frame held
/// past a vblank it was queued less than this before reads as on time:
/// `tests::a_flip_stamped_just_after_its_vblank_is_on_time`.
const FLIP_MARGIN: Duration = Duration::from_micros(200);

/// **How long a mode's vertical blank lasts**: the lines it scans and does
/// not show, as a share of its interval. The kernel stamps a flip as its
/// blank ends, when scanout starts, so a frame from idle that flips on time
/// is stamped up to an interval and a blank after it was queued, and a blank
/// can be longer than any fixed slack: 0.67 ms on CEA's 1080p60.
/// `tests::a_flip_from_idle_on_a_cea_1080p60_mode_is_judged_by_its_blank`.
pub(crate) fn vertical_blank(interval: Duration, shown: u16, total: u16) -> Duration {
    let hidden = u32::from(total.saturating_sub(shown));
    interval
        .checked_mul(hidden)
        .and_then(|lines| lines.checked_div(u32::from(total)))
        .unwrap_or(Duration::ZERO)
}

/// **Vblanks this screen should have flipped at and did not.**
///
/// A frame drawn for a vblank should flip on the next one, so every vblank in
/// between was lost: `tests::a_chained_frame_that_skipped_a_vblank_is_one_late`,
/// `tests::a_sequence_that_wrapped_is_not_four_billion_late`. A frame started
/// from idle or a client's commit has no vblank it was drawn for, so it is
/// late only if a whole interval passed between queueing and flipping, less
/// the screen's `blank` the flip is stamped at the end of — the GPU or the
/// fence held it: `tests::a_frame_from_idle_that_made_the_first_vblank_is_on_time`,
/// `tests::a_frame_held_past_a_vblank_by_its_fence_is_late`,
/// `tests::a_flip_from_idle_on_a_cea_1080p60_mode_is_judged_by_its_blank`.
pub(crate) fn vblanks_missed(
    queued: Queued,
    flipped: Flip,
    interval: Duration,
    blank: Duration,
) -> u32 {
    if let Some(trigger) = queued.after {
        return flipped.seq.wrapping_sub(trigger.seq).saturating_sub(1);
    }
    if interval.is_zero() {
        return 0;
    }
    let waited = flipped
        .at
        .saturating_sub(queued.at)
        .saturating_sub(blank.saturating_add(FLIP_MARGIN));
    u32::try_from(waited.as_nanos() / interval.as_nanos()).unwrap_or(u32::MAX)
}

/// CLOCK_MONOTONIC now, which is the clock the kernel stamps flips with.
pub(crate) fn monotonic_now() -> Duration {
    smithay::utils::Clock::<smithay::utils::Monotonic>::new()
        .now()
        .into()
}

/// A flip landed `late` vblanks late. Counted always:
/// `tests::a_late_flip_is_counted_with_the_knob_off`,
/// `tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`. With a trace
/// open, its record is written, and only then is `monitor` asked for its
/// name: `tests::a_traced_pass_and_its_flip_through_the_backends_calls`.
pub(crate) fn flipped(late: u32, queued: Queued, flip: Flip, monitor: impl FnOnce() -> String) {
    COUNTERS.with(|counters| {
        counters.flipped(late);
        if let Ok(mut trace) = counters.trace.try_borrow_mut()
            && let Some(trace) = trace.as_mut()
        {
            trace.flip(&flip_record(&queued, flip, late, &monitor()));
        }
    });
}

/// What the session's passes came to, counted whether or not `SOLIUM_PACING`
/// is set: `tests::a_miss_is_counted_with_the_knob_off`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub(crate) passes: u64,
    pub(crate) missed: u64,
    /// Vblanks a flip missed: `tests::a_late_flip_is_counted_with_the_knob_off`.
    pub(crate) late: u64,
}

/// The session's totals so far, on this thread.
pub(crate) fn totals() -> Totals {
    COUNTERS.with(Counters::totals)
}

/// One line of totals, at the end of a session: what a run with the knob off
/// is compared by. Then the trace's waiting records are written, as late,
/// and a parked report goes, as at any idle
/// (`tests::a_counted_pass_is_traced_with_its_gpu_time`,
/// `tests::idle_flushes_a_parked_report`).
pub(crate) fn summary() {
    let totals = totals();
    tracing::info!(
        passes = totals.passes,
        missed = totals.missed,
        late = totals.late,
        "pacing: render passes this session (one in which no monitor was ready to draw still counts), passes that overran the tightest monitor's frame, and vblanks a flip missed"
    );
    COUNTERS.with(|counters| {
        if let Some(line) = counters.end(Instant::now) {
            emit(&line);
        }
    });
}

/// Whether pacing is on. `SOLIUM_PACING`, read once.
pub(crate) fn enabled() -> bool {
    COUNTERS.with(Counters::decide)
}

/// A window was drawn into a texture of its own in this pass.
pub(crate) fn captured() {
    COUNTERS.with(Counters::capture);
}

/// A pass's GPU time, from the backend's timer.
pub(crate) fn gpu_resolved(pass: u64, gpu: crate::gputime::Gpu) {
    COUNTERS.with(|counters| {
        if let Some(line) = counters.gpu_resolved(pass, gpu) {
            emit(&line);
        }
    });
}

/// The loop drew nothing: the GPU has finished what was measured, so a report
/// waiting for it goes now (`tests::idle_flushes_a_parked_report`), and so
/// does one a flip made due since the last pass
/// (`tests::a_late_flip_before_idle_is_reported_at_idle`).
pub(crate) fn idle() {
    COUNTERS.with(|counters| {
        if let Some(line) = counters.idle(Instant::now) {
            emit(&line);
        }
    });
}

/// One report: the worst pass of a span, and what the span came to.
///
/// A value rather than a `tracing` call, so a test can read what would be
/// said: `tests::the_first_miss_reports_immediately`. Microseconds throughout,
/// as integers, so a field can be grepped, plotted or filtered without parsing
/// a sentence.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    pub(crate) pass: u64,
    pub(crate) total_us: u64,
    pub(crate) deadline_us: u64,
    pub(crate) monitor: String,
    pub(crate) missed: u64,
    /// Vblanks a flip missed in the span:
    /// `tests::a_late_flip_makes_a_report_due_without_a_cpu_miss`.
    pub(crate) late: u64,
    pub(crate) frames: u64,
    pub(crate) span_ms: u64,
    pub(crate) spent_us: [u64; Phase::COUNT],
    pub(crate) panes: u32,
    pub(crate) drew: u32,
    pub(crate) scenes: u32,
    pub(crate) animating: u32,
    pub(crate) rendered: u32,
    pub(crate) built: u32,
    pub(crate) rebound: u32,
    pub(crate) gpu: Option<crate::gputime::Gpu>,
    pub(crate) captures: u32,
    pub(crate) clocks: Option<crate::clocks::Clocks>,
    pub(crate) source: crate::clocks::Source,
    /// The pass's three costliest scenes, as `bar@DP-1=2140,rounded/Frame=410`:
    /// `tests::a_line_names_its_slowest_passes_costliest_scenes`.
    pub(crate) qml_top: String,
}

impl Line {
    fn of(worst: &Slow, frames: u64, missed: u64, late: u64, span: Duration) -> Self {
        // Saturating rather than truncating: a pass that somehow lasted longer
        // than half a million years should read as enormous, not as small.
        let micros = |duration: Duration| u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        Self {
            pass: worst.pass,
            total_us: micros(worst.total),
            deadline_us: micros(worst.deadline),
            monitor: worst.monitor.clone(),
            missed,
            late,
            frames,
            span_ms: micros(span) / 1_000,
            spent_us: worst.spent.map(|nanos| nanos / 1_000),
            panes: worst.panes,
            drew: worst.drew,
            scenes: worst.scenes,
            animating: worst.animating,
            rendered: worst.rendered,
            built: worst.built,
            rebound: worst.rebound,
            gpu: worst.gpu,
            captures: worst.captures,
            clocks: worst.clocks,
            source: crate::clocks::source(),
            qml_top: worst.qml_top.clone(),
        }
    }

    /// `ok`, `unsupported`, `unread` or `disjoint`. A line sent before its
    /// GPU time was read is `unread`, a word of its own because `late` on the
    /// same line counts late flips.
    /// `tests::every_pacing_line_carries_its_gpu_time_and_captures`.
    pub(crate) fn gpu_status(&self) -> &'static str {
        match self.gpu {
            Some(crate::gputime::Gpu::Ok(_)) => "ok",
            Some(crate::gputime::Gpu::Unsupported) => "unsupported",
            Some(crate::gputime::Gpu::Disjoint) => "disjoint",
            Some(crate::gputime::Gpu::Late) | None => "unread",
        }
    }

    /// Microseconds of GPU time for the pass, 0 unless the status is `ok`.
    /// `tests::every_pacing_line_carries_its_gpu_time_and_captures`.
    pub(crate) fn gpu_us(&self) -> u64 {
        match self.gpu {
            Some(crate::gputime::Gpu::Ok(sample)) => sample.total_ns / 1_000,
            _ => 0,
        }
    }

    /// Microseconds of it spent on captures, 0 unless the status is `ok`.
    /// `tests::every_pacing_line_carries_its_gpu_time_and_captures`.
    pub(crate) fn gpu_prep_us(&self) -> u64 {
        match self.gpu {
            Some(crate::gputime::Gpu::Ok(sample)) => sample.captures_ns / 1_000,
            _ => 0,
        }
    }
}

/// Pacing on, on this thread, for a test elsewhere, until this is dropped:
/// then as it was, with no pass left measured.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Measured {
    asked: bool,
    on: bool,
}

#[cfg(test)]
impl Drop for Measured {
    fn drop(&mut self) {
        COUNTERS.with(|counters| {
            counters.asked.set(self.asked);
            counters.on.set(self.on);
            counters.live.set(false);
            counters.mark.set(None);
        });
    }
}

/// Turn pacing on for this thread, for as long as the guard is held.
#[cfg(test)]
pub(crate) fn measured() -> Measured {
    COUNTERS.with(|counters| {
        let was = Measured {
            asked: counters.asked.get(),
            on: counters.on.get(),
        };
        counters.asked.set(true);
        counters.on.set(true);
        was
    })
}

/// How many scenes hold a label on this thread.
#[cfg(test)]
pub(crate) fn interned() -> usize {
    COUNTERS.with(|counters| {
        counters
            .labels
            .try_borrow()
            .map_or(0, |labels| labels.iter().flatten().count())
    })
}

/// A scene's label, while it holds one.
#[cfg(test)]
pub(crate) fn label(id: SceneId) -> Option<String> {
    COUNTERS.with(|counters| {
        let labels = counters.labels.try_borrow().ok()?;
        labels.get(usize::try_from(id.0).ok()?)?.clone()
    })
}

/// A scene's Qt time in the pass being measured.
#[cfg(test)]
pub(crate) fn spent(id: SceneId) -> u64 {
    COUNTERS.with(|counters| counters.spent_by(id))
}

/// Say a report. The one `tracing` call this module makes for one.
fn emit(line: &Line) {
    let phase = |which: Phase| line.spent_us[which.slot()];
    tracing::warn!(
        pass = line.pass,
        total_us = line.total_us,
        deadline_us = line.deadline_us,
        monitor = line.monitor,
        missed = line.missed,
        late = line.late,
        frames = line.frames,
        span_ms = line.span_ms,
        tick_us = phase(Phase::Tick),
        prep_us = phase(Phase::Prep),
        census_us = phase(Phase::Census),
        qml_us = phase(Phase::Qml),
        qml_top = line.qml_top.as_str(),
        elements_us = phase(Phase::Elements),
        gles_us = phase(Phase::Gles),
        commit_us = phase(Phase::Commit),
        settle_us = phase(Phase::Settle),
        loose_us = phase(Phase::Loose),
        gpu = line.gpu_status(),
        gpu_us = line.gpu_us(),
        gpu_prep_us = line.gpu_prep_us(),
        captures = line.captures,
        clocks = line.source.name(),
        gpu_mhz = line.clocks.map_or(0, |clocks| clocks.gpu_mhz),
        mem_mhz = line.clocks.map_or(0, |clocks| clocks.mem_mhz),
        pstate = line
            .clocks
            .and_then(|clocks| clocks.pstate)
            .map_or(-1, i32::from),
        panes = line.panes,
        drew = line.drew,
        scenes = line.scenes,
        animating = line.animating,
        rendered = line.rendered,
        built = line.built,
        rebound = line.rebound,
        "PACING"
    );
}

/// Passes a record waits for its GPU time before it goes as late.
/// `tests::a_pass_record_that_waits_too_long_goes_out_as_late`.
const TRACE_WAIT: usize = 16;

/// What the trace buffers before a write: 30 to 120 KB a second at 60 to 260
/// passes, so a flush a second is the only write.
/// `tests::the_trace_is_buffered_and_flushed_once_a_second`.
const TRACE_BUFFER: usize = 256 * 1024;

/// Whether pacing is on: either knob. `tests::the_trace_turns_pacing_on`.
const fn knob(pacing: bool, trace: bool) -> bool {
    pacing || trace
}

/// The per-pass trace: `SOLIUM_TRACE`'s file.
///
/// A pass's record is written once its GPU time is in
/// (`tests::a_pass_record_is_written_when_its_gpu_time_resolves`), or as
/// late sixteen passes on or when the session ends
/// (`tests::a_pass_record_that_waits_too_long_goes_out_as_late`); a flip's
/// when it lands (`tests::a_flip_record_is_written_at_the_vblank`).
/// Buffered and flushed once a second, at idle and at exit, and never
/// `O_DSYNC`, unlike the session log:
/// `tests::the_trace_is_buffered_and_flushed_once_a_second`.
struct Trace {
    out: std::io::BufWriter<Box<dyn std::io::Write>>,
    waiting: std::collections::VecDeque<(u64, String)>,
    flushed: Instant,
}

impl std::fmt::Debug for Trace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trace")
            .field("waiting", &self.waiting.len())
            .finish_non_exhaustive()
    }
}

impl Trace {
    fn new(out: Box<dyn std::io::Write>, now: Instant) -> Self {
        Self {
            out: std::io::BufWriter::with_capacity(TRACE_BUFFER, out),
            waiting: std::collections::VecDeque::with_capacity(TRACE_WAIT + 1),
            flushed: now,
        }
    }

    fn open(path: &std::path::Path, now: Instant) -> Option<Self> {
        match std::fs::File::create(path) {
            Ok(file) => Some(Self::new(Box::new(file), now)),
            Err(err) => {
                tracing::warn!(?err, path = %path.display(), "SOLIUM_TRACE could not be opened; no trace");
                None
            }
        }
    }

    /// A pass's record, still missing its GPU time and its closing brace.
    fn pass(&mut self, pass: u64, record: String) {
        self.waiting.push_back((pass, record));
        if self.waiting.len() > TRACE_WAIT
            && let Some((_, record)) = self.waiting.pop_front()
        {
            self.write(&record, None);
        }
    }

    /// A pass's GPU time is in: its record goes.
    fn resolved(&mut self, pass: u64, gpu: Option<crate::gputime::Gpu>) {
        if let Some(at) = self
            .waiting
            .iter()
            .position(|(waiting, _)| *waiting == pass)
            && let Some((_, record)) = self.waiting.remove(at)
        {
            self.write(&record, gpu);
        }
    }

    fn flip(&mut self, record: &str) {
        let _ = writeln!(self.out, "{record}");
    }

    /// Flush once a second.
    fn tick(&mut self, now: Instant) {
        if now.saturating_duration_since(self.flushed) >= REPORT_EVERY {
            let _ = std::io::Write::flush(&mut self.out);
            self.flushed = now;
        }
    }

    /// What is written goes to the file; what waits for its GPU time keeps
    /// waiting:
    /// `tests::a_record_waiting_as_the_loop_goes_idle_keeps_waiting_for_its_gpu_time`.
    fn flush(&mut self) {
        let _ = std::io::Write::flush(&mut self.out);
    }

    /// Everything still waiting goes as late, and the buffer is flushed.
    fn close(&mut self) {
        while let Some((_, record)) = self.waiting.pop_front() {
            self.write(&record, None);
        }
        self.flush();
    }

    fn write(&mut self, record: &str, gpu: Option<crate::gputime::Gpu>) {
        let _ = writeln!(self.out, "{record}{}", gpu_fields(gpu));
    }
}

/// A record's GPU half, closing brace included.
/// `tests::every_pass_record_carries_the_documented_fields`.
fn gpu_fields(gpu: Option<crate::gputime::Gpu>) -> String {
    use crate::gputime::{Gpu, GpuSample};
    let (status, sample) = match gpu {
        Some(Gpu::Ok(sample)) => ("ok", sample),
        Some(Gpu::Unsupported) => ("unsupported", GpuSample::default()),
        Some(Gpu::Disjoint) => ("disjoint", GpuSample::default()),
        Some(Gpu::Late) | None => ("late", GpuSample::default()),
    };
    let [a, b, c, d] = sample.outputs_ns.map(|nanos| nanos / 1_000);
    format!(
        r#","gpu":"{status}","gpu_us":{},"gpu_prep_us":{},"gpu_effects_us":{},"gpu_out_us":[{a},{b},{c},{d}]}}"#,
        sample.total_ns / 1_000,
        sample.captures_ns / 1_000,
        sample.effects_ns / 1_000
    )
}

/// A flip's record. `tests::a_flip_record_is_written_at_the_vblank`.
fn flip_record(queued: &Queued, flip: Flip, late: u32, monitor: &str) -> String {
    format!(
        r#"{{"flip":{},"monitor":{},"seq":{},"at_ns":{},"queued_ns":{},"late":{late}}}"#,
        queued.pass,
        crate::scripted::json_string(monitor),
        flip.seq,
        flip.at.as_nanos(),
        queued.at.as_nanos()
    )
}

#[cfg(test)]
mod tests {
    use super::{
        Counters, Flip, Frame, Line, Phase, Queued, SceneId, Totals, Trace, vblanks_missed,
        vertical_blank,
    };
    use crate::gputime::{Gpu, GpuSample};
    use std::time::{Duration, Instant};

    /// The deadline of a 260 Hz monitor, which every test here is written
    /// against: 3.846 ms.
    fn at_260() -> Duration {
        Duration::from_nanos(3_846_153)
    }

    /// **A miss is counted with the knob off**, and nothing is reported. The
    /// totals are what two runs are compared by, so they cannot depend on the
    /// knob whose cost they are there to measure.
    #[test]
    fn a_miss_is_counted_with_the_knob_off() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        let line = counters.finish_at(start, start + ms(5), false, 3, 1);
        assert!(line.is_none(), "the knob is off, so nothing is said");
        assert_eq!(
            counters.totals(),
            Totals {
                passes: 1,
                missed: 1,
                late: 0
            }
        );
    }

    /// The deadline is a ceiling, not a target: a pass that fits, and one that
    /// lands exactly on it, are not misses. Pins the `<=`.
    #[test]
    fn a_pass_inside_its_deadline_is_not_a_miss() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        assert!(
            counters
                .finish_at(start, start + Duration::from_micros(3_800), true, 1, 1)
                .is_none()
        );
        assert!(
            counters
                .finish_at(start, start + at_260(), true, 1, 2)
                .is_none()
        );
        assert_eq!(
            counters.totals(),
            Totals {
                passes: 2,
                missed: 0,
                late: 0
            }
        );
    }

    /// With the knob off the monitor's name is never asked for: `Output::name`
    /// allocates, and this runs once a pass.
    #[test]
    fn the_deadline_takes_no_name_with_the_knob_off() {
        let frame = Frame {
            on: false,
            started: Some(Instant::now()),
            pass: 1,
        };
        frame.deadline(at_260(), || panic!("the name was taken with the knob off"));
        super::COUNTERS.with(|counters| assert_eq!(counters.deadline.get(), at_260()));
    }

    /// **A pass with the knob off is counted from `frame` to `finish`**, the
    /// way the backends call them, not through `finish_at` alone: the pass is
    /// counted, its miss with it, its serial runs from 1, and a pass whose
    /// backend named no deadline is not judged against the last one's.
    #[test]
    fn a_pass_with_the_knob_off_is_counted_from_frame_to_finish() {
        super::COUNTERS.with(|counters| {
            counters.asked.set(true);
            counters.on.set(false);
        });
        let first = super::frame();
        assert_eq!(first.serial(), 1, "the serial counts from 1");
        // A deadline no pass can meet, so this one is a miss however fast.
        first.deadline(Duration::from_nanos(1), || {
            panic!("the name was taken with the knob off")
        });
        std::thread::sleep(ms(1));
        first.finish(3);
        assert_eq!(
            super::totals(),
            Totals {
                passes: 1,
                missed: 1,
                late: 0
            }
        );

        let second = super::frame();
        assert_eq!(second.pass, 2);
        std::thread::sleep(ms(1));
        second.finish(3);
        assert_eq!(
            super::totals(),
            Totals {
                passes: 2,
                missed: 1,
                late: 0
            },
            "a pass that named no deadline was judged against the last one's"
        );
    }

    /// **A sustained stall costs one line a second, not one a frame**, now
    /// driven through the rule itself rather than through a copy of it. Sixty
    /// seconds of a compositor missing every frame at 260 Hz is 15,600 slow
    /// passes; at a line each, into a log opened `O_DSYNC`, the diagnostic is
    /// the outage.
    #[test]
    fn a_sustained_stall_is_one_line_a_second() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        let mut lines = 0;
        for pass in 0..15_600_u64 {
            let begun = start + Duration::from_nanos(pass * 3_846_153);
            if counters
                .finish_at(begun, begun + ms(5), true, 1, pass)
                .is_some()
            {
                lines += 1;
            }
        }
        assert_eq!(
            lines, 60,
            "sixty seconds of solid stall should be sixty lines"
        );
    }

    /// And the first one goes out at once, rather than a second late, carrying
    /// the pass it describes.
    #[test]
    fn the_first_miss_reports_immediately() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        let line = counters.finish_at(start, start + ms(17), true, 2, 41);
        assert_eq!(
            line.map(|line| (line.pass, line.total_us, line.panes)),
            Some((41, 17_000, 2))
        );
    }

    /// A hiccup every few seconds is reported every time: the limit is a
    /// ceiling on the rate, not a sampling interval.
    #[test]
    fn an_occasional_miss_is_never_swallowed() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        for second in 0..30_u64 {
            let begun = start + Duration::from_secs(second * 5);
            assert!(
                counters
                    .finish_at(begun, begun + ms(6), true, 1, second)
                    .is_some(),
                "a miss five seconds after the last report was dropped"
            );
        }
    }

    /// What a pass's GPU time came to in the tests below.
    fn timed() -> Gpu {
        let mut sample = GpuSample {
            total_ns: 912_000,
            captures_ns: 640_000,
            ..GpuSample::default()
        };
        sample.outputs_ns[0] = 272_000;
        Gpu::Ok(sample)
    }

    /// **A report waits for its pass's GPU time**, which is read passes later:
    /// the line that goes out carries it.
    #[test]
    fn a_report_waits_for_its_passes_gpu_time() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        let due = counters
            .finish_at(start, start + ms(5), true, 1, 7)
            .expect("a first miss is due");
        assert_eq!(due.gpu, None, "pass 7's GPU time cannot be in yet");
        counters.park(due, 7);
        assert!(counters.waited(8).is_none(), "a pass later it still waits");
        let sent = counters
            .gpu_resolved(7, timed())
            .expect("sent once its GPU time is in");
        assert_eq!(
            (sent.gpu_status(), sent.gpu_us(), sent.gpu_prep_us()),
            ("ok", 912, 640)
        );
        assert!(counters.flush().is_none(), "and sent once");
    }

    /// A GPU time that never comes does not hold the report for ever.
    #[test]
    fn a_report_goes_out_without_gpu_time_after_eight_passes() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        let due = counters
            .finish_at(start, start + ms(5), true, 1, 7)
            .expect("due");
        counters.park(due, 7);
        assert!(counters.waited(14).is_none());
        let sent = counters.waited(15).expect("eight passes on, it goes");
        assert_eq!((sent.pass, sent.gpu_status()), (7, "unread"));
    }

    /// The loop going idle sends what is parked: the last miss before a pause
    /// is the one somebody is looking for.
    #[test]
    fn idle_flushes_a_parked_report() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        let due = counters
            .finish_at(start, start + ms(5), true, 1, 7)
            .expect("due");
        counters.park(due, 7);
        assert_eq!(
            counters.idle(|| start + ms(6)).map(|line| line.pass),
            Some(7)
        );
    }

    /// **A late flip seen as the loop goes idle is reported then**, not when
    /// the next pass comes, which may be minutes later and would stretch the
    /// line's span over the idle time. Once, and inside the limit like any
    /// other report: a second late flip within the second waits, and with no
    /// pass since the last line it has none to show, so it waits for the
    /// next pass.
    #[test]
    fn a_late_flip_before_idle_is_reported_at_idle() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        assert!(
            counters
                .finish_at(start, start + ms(2), true, 1, 1)
                .is_none(),
            "on time"
        );
        counters.flipped(1);
        let line = counters
            .idle(|| start + ms(6))
            .expect("the late flip goes at idle");
        assert_eq!((line.pass, line.missed, line.late), (1, 0, 1));
        assert!(counters.idle(|| start + ms(7)).is_none(), "and goes once");
        counters.flipped(1);
        assert!(
            counters.idle(|| start + ms(9)).is_none(),
            "inside the limit"
        );
        assert!(
            counters.idle(|| start + Duration::from_secs(2)).is_none(),
            "no pass since the line, so none to show"
        );
        let later = start + Duration::from_secs(3);
        let next = counters
            .finish_at(later, later + ms(1), true, 1, 2)
            .expect("the late flip waited for it");
        assert_eq!((next.pass, next.late), (2, 1));
    }

    /// The worst pass of a span can resolve before the span's report is due;
    /// its GPU time is kept and goes out with it.
    #[test]
    fn a_gpu_time_that_came_before_its_report_goes_out_with_it() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        counters.reported.set(Some(start));
        assert!(
            counters
                .finish_at(start, start + ms(5), true, 1, 3)
                .is_none(),
            "inside the limit"
        );
        assert!(
            counters.gpu_resolved(3, timed()).is_none(),
            "nothing is parked yet"
        );
        let later = start + Duration::from_secs(1);
        let due = counters
            .finish_at(later, later + ms(4), true, 1, 4)
            .expect("due a second on");
        assert_eq!((due.pass, due.gpu_status(), due.gpu_us()), (3, "ok", 912));
    }

    /// **A GPU that cannot time itself says so at once.** Without the
    /// extension the timer answers `unsupported` as a pass begins, before
    /// that pass has a report to carry it; the report goes out with it
    /// rather than being held eight passes and called `unread`.
    #[test]
    fn a_gpu_time_in_before_its_pass_ends_goes_out_with_it() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        // Pass 1, as `frame` numbers it, is being measured, and fits.
        assert!(counters.gpu_resolved(1, Gpu::Unsupported).is_none());
        assert!(
            counters
                .finish_at(start, start + ms(1), true, 1, 1)
                .is_none()
        );
        // Pass 2 misses, and its answer came as it began.
        assert!(counters.gpu_resolved(2, Gpu::Unsupported).is_none());
        let due = counters
            .finish_at(start, start + ms(5), true, 1, 2)
            .expect("a first miss is due");
        assert_eq!((due.pass, due.gpu_status()), (2, "unsupported"));
        // An answer is its own pass's only.
        let later = start + Duration::from_secs(1);
        let next = counters
            .finish_at(later, later + ms(5), true, 1, 3)
            .expect("due a second on");
        assert_eq!(next.gpu, None);
    }

    /// **Every PACING line carries its GPU time, its status, its captures and
    /// its clocks**, and a status that is not `ok` reads as zero microseconds.
    #[test]
    fn every_pacing_line_carries_its_gpu_time_and_captures() {
        let line = |gpu: Option<Gpu>| Line {
            pass: 1,
            total_us: 0,
            deadline_us: 0,
            monitor: String::new(),
            missed: 0,
            late: 0,
            frames: 0,
            span_ms: 0,
            spent_us: [0; Phase::COUNT],
            panes: 5,
            drew: 1,
            scenes: 0,
            animating: 0,
            rendered: 0,
            built: 0,
            rebound: 0,
            gpu,
            captures: 5,
            clocks: Some(crate::clocks::Clocks {
                gpu_mhz: 1080,
                mem_mhz: 5001,
                pstate: Some(3),
            }),
            source: crate::clocks::Source::Nvml,
            qml_top: String::new(),
        };
        assert_eq!(line(Some(timed())).gpu_status(), "ok");
        assert_eq!(line(Some(Gpu::Unsupported)).gpu_status(), "unsupported");
        assert_eq!(line(Some(Gpu::Late)).gpu_status(), "unread");
        assert_eq!(line(None).gpu_status(), "unread");
        assert_eq!(line(Some(Gpu::Disjoint)).gpu_status(), "disjoint");
        assert_eq!(
            (
                line(Some(timed())).gpu_us(),
                line(Some(timed())).gpu_prep_us()
            ),
            (912, 640)
        );
        let disjoint = line(Some(Gpu::Disjoint));
        assert_eq!(
            (
                disjoint.gpu_us(),
                disjoint.gpu_prep_us(),
                line(None).captures
            ),
            (0, 0, 5)
        );
        let held = line(None);
        assert_eq!(
            (held.source.name(), held.clocks.map(|c| c.gpu_mhz)),
            ("nvml", Some(1080))
        );
    }

    /// **A pass takes its clocks from the sampler as it ends**, through
    /// `frame` and `finish` as the backends call them: what the sampler last
    /// left, and with nothing sampling, `none` and no clocks. The one test
    /// that writes the sampler's atomics, and it clears them.
    #[test]
    fn a_pass_takes_its_clocks_from_the_sampler() {
        use crate::clocks::{Clocks, Source};
        super::COUNTERS.with(|counters| {
            counters.asked.set(true);
            counters.on.set(true);
        });
        // A pass that misses, whose line is parked for its GPU time.
        let missed = || {
            let pass = super::frame();
            pass.deadline(Duration::from_nanos(1), String::new);
            std::thread::sleep(ms(1));
            pass.finish(1);
            super::COUNTERS.with(|counters| {
                counters.reported.set(None);
                counters.flush()
            })
        };
        let unsampled = missed().expect("a miss is reported");
        assert_eq!((unsampled.source.name(), unsampled.clocks), ("none", None));
        let clocks = Clocks {
            gpu_mhz: 1080,
            mem_mhz: 5001,
            pstate: Some(3),
        };
        crate::clocks::sampled(Source::Nvml, clocks);
        let sampled = missed();
        crate::clocks::sampled(Source::None, Clocks::default());
        let sampled = sampled.expect("a miss is reported");
        assert_eq!(
            (sampled.source.name(), sampled.clocks),
            ("nvml", Some(clocks))
        );
    }

    /// **A line carries the clocks of its pass, not of the moment it is
    /// made.** A line reports the slowest pass of its span, which after an
    /// idle spell can be minutes old, by when the GPU has dropped to its idle
    /// clocks: those beside that pass's cost would read as a slow pass on a
    /// slow GPU.
    #[test]
    fn a_line_made_long_after_its_pass_carries_that_passes_clocks() {
        use crate::clocks::Clocks;
        let busy = Clocks {
            gpu_mhz: 1950,
            mem_mhz: 10_501,
            pstate: Some(0),
        };
        let idle = Clocks {
            gpu_mhz: 210,
            mem_mhz: 405,
            pstate: Some(8),
        };
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        // The last frame of an animation: on time, and the span's slowest.
        counters.clocks.set(Some(busy));
        assert!(
            counters
                .finish_at(start, start + ms(3), true, 1, 500)
                .is_none()
        );
        // Five minutes idle, then a keystroke's frame, whose flip is late,
        // and the pass after it, which makes the line.
        let later = start + Duration::from_secs(300);
        counters.clocks.set(Some(idle));
        assert!(
            counters
                .finish_at(later, later + ms(1), true, 1, 501)
                .is_none()
        );
        counters.flipped(1);
        let line = counters
            .finish_at(later + ms(4), later + ms(5), true, 1, 502)
            .expect("a late flip is reported");
        assert_eq!((line.pass, line.clocks), (500, Some(busy)));
    }

    /// The vertical blank of the 260 Hz mode these tests are written
    /// against: 160 of its 1,600 lines, 0.38 ms.
    fn blank_260() -> Duration {
        vertical_blank(at_260(), 1440, 1600)
    }

    fn flip(seq: u32, at_us: u64) -> Flip {
        Flip {
            seq,
            at: Duration::from_micros(at_us),
        }
    }

    /// A frame drawn for the vblank before it that flipped one vblank further
    /// on lost exactly that one.
    #[test]
    fn a_chained_frame_that_skipped_a_vblank_is_one_late() {
        let queued = Queued {
            at: Duration::from_micros(10_500),
            after: Some(flip(100, 10_000)),
            pass: 1,
        };
        assert_eq!(
            vblanks_missed(queued, flip(102, 17_692), at_260(), blank_260()),
            1
        );
        assert_eq!(
            vblanks_missed(queued, flip(101, 13_846), at_260(), blank_260()),
            0,
            "the next vblank is on time"
        );
    }

    /// A frame started from idle or from a client's commit is late only if a
    /// whole vblank passed between queueing it and its flip.
    #[test]
    fn a_frame_from_idle_that_made_the_first_vblank_is_on_time() {
        let queued = Queued {
            at: Duration::from_micros(50_000),
            after: None,
            pass: 1,
        };
        assert_eq!(
            vblanks_missed(queued, flip(7, 51_000), at_260(), blank_260()),
            0
        );
    }

    /// Held past a vblank by its fence: late, though the CPU was on time.
    #[test]
    fn a_frame_held_past_a_vblank_by_its_fence_is_late() {
        let queued = Queued {
            at: Duration::from_micros(50_000),
            after: None,
            pass: 1,
        };
        assert_eq!(
            vblanks_missed(queued, flip(7, 54_615), at_260(), blank_260()),
            1
        );
    }

    /// Queued 0.1 ms after one vblank began and flipped on the next,
    /// stamped as that one's blank ends, 0.38 ms in: a little more than an
    /// interval by the stamps, and on time. And the margin's price: queued
    /// 0.1 ms before a vblank and held past it, a frame reads as on time
    /// too.
    #[test]
    fn a_flip_stamped_just_after_its_vblank_is_on_time() {
        let queued = Queued {
            at: Duration::from_micros(50_000),
            after: None,
            pass: 1,
        };
        assert_eq!(
            vblanks_missed(queued, flip(7, 54_131), at_260(), blank_260()),
            0
        );
        // The vblank it missed began at 50.100 ms; the next begins at
        // 53.946 ms, and its blank ends at 54.331 ms.
        assert_eq!(
            vblanks_missed(queued, flip(7, 54_331), at_260(), blank_260()),
            0,
            "inside the margin"
        );
    }

    /// **However long a mode's blank, a flip from idle is judged by it.** A
    /// flip is stamped as its vertical blank ends, and CEA's 1080p60 blanks
    /// 45 of its 1,125 lines, 0.67 ms: a frame queued 0.05 ms after one
    /// vblank began flips on time on the next, stamped an interval and 0.62 ms
    /// later; one queued 0.25 ms before a vblank and held past it by its
    /// fence is one late.
    #[test]
    fn a_flip_from_idle_on_a_cea_1080p60_mode_is_judged_by_its_blank() {
        let interval = Duration::from_nanos(16_666_666);
        let blank = vertical_blank(interval, 1080, 1125);
        assert_eq!(blank, Duration::from_nanos(666_666));
        let queued = Queued {
            at: Duration::from_micros(50_000),
            after: None,
            pass: 1,
        };
        // A vblank began at 49.950 ms; the next begins at 66.617 ms, and its
        // blank ends at 67.283 ms.
        assert_eq!(
            vblanks_missed(queued, flip(8, 67_283), interval, blank),
            0,
            "queued just after a vblank began, and on time on the next"
        );
        // A vblank begins at 50.250 ms, and the frame misses it; the next
        // begins at 66.917 ms, and its blank ends at 67.583 ms.
        assert_eq!(
            vblanks_missed(queued, flip(8, 67_583), interval, blank),
            1,
            "held past the vblank it was queued for"
        );
    }

    /// The kernel's sequence is a `u32` and wraps; a wrap is not four billion
    /// vblanks.
    #[test]
    fn a_sequence_that_wrapped_is_not_four_billion_late() {
        let queued = Queued {
            at: Duration::ZERO,
            after: Some(flip(u32::MAX, 0)),
            pass: 1,
        };
        assert_eq!(
            vblanks_missed(queued, flip(1, 7_692), at_260(), blank_260()),
            1
        );
    }

    /// **A late flip is counted with the knob off**, into the totals and not
    /// into a report: the totals are what a run with the knob off is
    /// compared by.
    #[test]
    fn a_late_flip_is_counted_with_the_knob_off() {
        let counters = counters();
        counters.on.set(false);
        counters.deadline.set(at_260());
        counters.flipped(2);
        let start = Instant::now();
        assert!(
            counters
                .finish_at(start, start + ms(2), false, 1, 1)
                .is_none(),
            "the knob is off, so nothing is said"
        );
        assert_eq!(counters.late.get(), 0, "and nothing waits to be said");
        assert_eq!(
            counters.totals(),
            Totals {
                passes: 1,
                missed: 0,
                late: 2
            }
        );
    }

    /// **A late flip makes a report due with no CPU miss at all** — the
    /// GPU-bound stutter, which reads `late>0`, `missed=0` and a high `gpu_us`.
    #[test]
    fn a_late_flip_makes_a_report_due_without_a_cpu_miss() {
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        assert!(
            counters
                .finish_at(start, start + ms(2), true, 1, 1)
                .is_none(),
            "on time"
        );
        counters.flipped(1);
        let due = counters
            .finish_at(start + ms(4), start + ms(6), true, 1, 2)
            .expect("a late flip is reported");
        assert_eq!((due.missed, due.late), (0, 1));
        assert_eq!(
            counters.totals(),
            Totals {
                passes: 2,
                missed: 0,
                late: 1
            }
        );
    }

    /// A capture is counted only inside a measured pass: the GPU pre-flight
    /// and a scene built between passes belong to nothing.
    #[test]
    fn a_capture_is_counted_only_inside_a_measured_pass() {
        let counters = counters();
        counters.live.set(false);
        counters.capture();
        assert_eq!(counters.captures.get(), 0);
        counters.live.set(true);
        counters.capture();
        assert_eq!(counters.captures.get(), 1);
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

    /// **Qt's time in a pass is charged to the scene that spent it.**
    #[test]
    fn qml_time_is_charged_to_the_scene_that_spent_it() {
        let counters = counters();
        let bar = counters.intern("bar@DP-1");
        let frame = counters.intern("rounded/Frame");
        counters.charge(bar, 3_000);
        counters.charge(frame, 5_000);
        counters.charge(bar, 1_000);
        assert_eq!(counters.spent_by(bar), 4_000);
        assert_eq!(counters.spent_by(frame), 5_000);
    }

    /// A ninth scene in one pass is summed into "other", rather than allocated
    /// a slot on the hot path.
    #[test]
    fn more_scenes_than_slots_land_in_other() {
        let counters = counters();
        let scenes: Vec<SceneId> = (0..=super::SCENE_SLOTS)
            .map(|n| counters.intern(&format!("s{n}")))
            .collect();
        for scene in &scenes {
            counters.charge(*scene, 10);
        }
        assert_eq!(counters.scene_other.get(), 10);
    }

    /// The report names the three costliest scenes, costliest first.
    #[test]
    fn the_report_names_the_three_costliest_scenes() {
        let counters = counters();
        for (label, nanos) in [
            ("cursor", 35_000),
            ("bar@DP-1", 2_140_000),
            ("wallpaper", 9_000),
            ("rounded/Frame", 410_000),
        ] {
            let id = counters.intern(label);
            counters.charge(id, nanos);
        }
        assert_eq!(counters.top(), "bar@DP-1=2140,rounded/Frame=410,cursor=35");
    }

    /// A scene that is freed gives its id back, so a session that opens ten
    /// thousand windows keeps ten thousand labels' worth of nothing.
    #[test]
    fn a_forgotten_scene_gives_its_id_back() {
        let counters = counters();
        let first = counters.intern("rounded/Frame");
        counters.forget(first);
        let second = counters.intern("top/Frame");
        assert_eq!(first, second);
        counters.charge(second, 1_000);
        assert_eq!(counters.top(), "top/Frame=1");
    }

    /// With the knob off nothing is interned, so nothing is allocated, and a
    /// charge to no scene is no charge.
    #[test]
    fn off_charges_nothing() {
        let counters = counters();
        counters.on.set(false);
        let id = counters.intern("bar@DP-1");
        assert_eq!(id, SceneId::NONE);
        counters.charge(id, 1_000);
        assert_eq!(counters.top(), "");
    }

    /// A scene's label is its file and the folder it is in.
    #[test]
    fn a_scene_is_labelled_by_its_folder_and_file() {
        let label = |path: &str| super::label_of(std::path::Path::new(path));
        assert_eq!(
            label("/usr/share/solium/qml/panes/rounded/Ring.qml"),
            "rounded/Ring"
        );
        assert_eq!(label("wallpaper.qml"), "wallpaper");
    }

    /// **A line names its slowest pass's costliest scenes**: the snapshot
    /// keeps them, and the line carries them.
    #[test]
    fn a_line_names_its_slowest_passes_costliest_scenes() {
        let counters = counters();
        counters.deadline.set(at_260());
        let bar = counters.intern("bar@DP-1");
        let frame = counters.intern("rounded/Frame");
        counters.charge(frame, 410_000);
        counters.charge(bar, 2_140_000);
        let start = Instant::now();
        let line = counters
            .finish_at(start, start + ms(5), true, 1, 1)
            .expect("a first miss is due");
        assert_eq!(line.qml_top, "bar@DP-1=2140,rounded/Frame=410");
    }

    /// **A scene's span charges it and Qt's phase alike**, through `frame`,
    /// `span` and `qml` as the render path calls them: a scene built inside
    /// the compositor's `elements` is charged its whole time, all of which is
    /// Qt's and none of which is the compositor's.
    #[test]
    fn a_scenes_span_charges_it_and_the_phase_alike() {
        super::COUNTERS.with(|counters| {
            counters.asked.set(true);
            counters.on.set(true);
        });
        let pass = super::frame();
        let id = super::scene_id("rounded/Frame");
        {
            let _elements = super::span(Phase::Elements);
            let _qml = super::qml(id);
            std::thread::sleep(ms(2));
        }
        let (scene, qml, elements) = super::COUNTERS.with(|counters| {
            (
                counters.spent_by(id),
                counters.spent[Phase::Qml.slot()].get(),
                counters.spent[Phase::Elements.slot()].get(),
            )
        });
        pass.finish(0);
        assert!(scene >= 2_000_000, "the scene was charged {scene} ns");
        assert_eq!(scene, qml, "the scene and Qt's phase disagree");
        assert!(
            elements < scene,
            "the scene's time was charged to the compositor: {elements} ns"
        );
    }

    /// **Each pass charges its scenes afresh**: what a scene spent in one
    /// pass is not carried into the next.
    #[test]
    fn each_pass_charges_its_scenes_afresh() {
        super::COUNTERS.with(|counters| {
            counters.asked.set(true);
            counters.on.set(true);
        });
        let id = super::scene_id("bar@DP-1");
        let first = super::frame();
        super::COUNTERS.with(|counters| counters.charge(id, 1_000));
        first.finish(0);
        let second = super::frame();
        let carried = super::COUNTERS.with(|counters| counters.spent_by(id));
        second.finish(0);
        assert_eq!(carried, 0, "the last pass's time was carried");
    }

    /// A sink a test can read back.
    #[derive(Clone, Debug, Default)]
    struct Shared(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);

    impl std::io::Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn lines(&self) -> Vec<String> {
            String::from_utf8_lossy(&self.0.borrow())
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    /// A record's value for `name` as written, up to the next comma or brace.
    fn field<'a>(record: &'a str, name: &str) -> &'a str {
        record
            .split_once(&format!("\"{name}\":"))
            .and_then(|(_, rest)| rest.split([',', '}']).next())
            .unwrap_or("")
    }

    /// **A pass's record goes out once its GPU time is in**, and not before:
    /// the record is the pass and its GPU time, together.
    #[test]
    fn a_pass_record_is_written_when_its_gpu_time_resolves() {
        let sink = Shared::default();
        let start = Instant::now();
        let mut trace = Trace::new(Box::new(sink.clone()), start);
        trace.pass(7, r#"{"pass":7,"total_us":3100"#.to_owned());
        trace.tick(start + Duration::from_secs(2));
        assert!(sink.lines().is_empty(), "written before its GPU time came");
        trace.resolved(7, Some(timed()));
        trace.tick(start + Duration::from_secs(4));
        let lines = sink.lines();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with(r#"{"pass":7,"#)
                && lines[0].contains(r#""gpu":"ok","gpu_us":912"#)
                && lines[0].ends_with('}')
        );
    }

    /// One whose GPU time never comes goes out as late, sixteen passes on.
    #[test]
    fn a_pass_record_that_waits_too_long_goes_out_as_late() {
        let sink = Shared::default();
        let start = Instant::now();
        let mut trace = Trace::new(Box::new(sink.clone()), start);
        for pass in 1..=17 {
            trace.pass(pass, format!(r#"{{"pass":{pass}"#));
        }
        trace.close();
        let lines = sink.lines();
        assert!(lines[0].starts_with(r#"{"pass":1,"#) && lines[0].contains(r#""gpu":"late""#));
        assert_eq!(lines.len(), 17, "closing writes the rest as late");
    }

    /// A flip's record is written when the flip lands.
    #[test]
    fn a_flip_record_is_written_at_the_vblank() {
        let sink = Shared::default();
        let start = Instant::now();
        let mut trace = Trace::new(Box::new(sink.clone()), start);
        let queued = Queued {
            at: Duration::from_micros(50_000),
            after: None,
            pass: 9,
        };
        trace.flip(&super::flip_record(
            &queued,
            Flip {
                seq: 3,
                at: Duration::from_micros(54_615),
            },
            1,
            "DP-1",
        ));
        trace.close();
        assert_eq!(
            sink.lines(),
            vec![
                r#"{"flip":9,"monitor":"DP-1","seq":3,"at_ns":54615000,"queued_ns":50000000,"late":1}"#
                    .to_owned()
            ]
        );
    }

    /// The fields of a pass record, in order. `dev/pacing-summary.py` reads
    /// them and `dev/README.md` lists them; this list is what the record is
    /// held to.
    const PASS_FIELDS: &[&str] = &[
        "pass",
        "t_ns",
        "total_us",
        "deadline_us",
        "monitor",
        "missed",
        "tick_us",
        "prep_us",
        "census_us",
        "qml_us",
        "elements_us",
        "gles_us",
        "commit_us",
        "settle_us",
        "loose_us",
        "captures",
        "panes",
        "drew",
        "scenes",
        "animating",
        "rendered",
        "built",
        "rebound",
        "qml",
        "clocks",
        "gpu_mhz",
        "mem_mhz",
        "pstate",
        "gpu",
        "gpu_us",
        "gpu_prep_us",
        "gpu_effects_us",
        "gpu_out_us",
    ];

    /// **A pass record carries every field `dev/pacing-summary.py` reads.**
    #[test]
    fn every_pass_record_carries_the_documented_fields() {
        let counters = counters();
        counters.deadline.set(at_260());
        let record = format!(
            "{}{}",
            counters.pass_record(1, ms(5), true),
            super::gpu_fields(Some(timed()))
        );
        for field in PASS_FIELDS {
            assert!(
                record.contains(&format!("\"{field}\":")),
                "{field} is missing from {record}"
            );
        }
    }

    /// **Scenes that share a label are one name in a record's `qml`, their
    /// time summed**: every window's frame in a style is built from the same
    /// file, and a JSON object holds a name once, so a reader would keep the
    /// last window's time alone.
    #[test]
    fn scenes_that_share_a_label_are_summed_in_a_record() {
        let counters = counters();
        let first = counters.intern("rounded/Pane");
        let second = counters.intern("rounded/Pane");
        let wallpaper = counters.intern("wallpaper/Wallpaper");
        counters.charge(first, 400_000);
        counters.charge(wallpaper, 50_000);
        counters.charge(second, 300_000);
        let record = counters.pass_record(1, ms(5), false);
        assert!(
            record.contains(r#""qml":{"rounded/Pane":700,"wallpaper/Wallpaper":50},"#),
            "{record}"
        );
    }

    /// Buffered, and flushed once a second: never a synchronous write per pass.
    #[test]
    fn the_trace_is_buffered_and_flushed_once_a_second() {
        let sink = Shared::default();
        let start = Instant::now();
        let mut trace = Trace::new(Box::new(sink.clone()), start);
        for pass in 1..=100 {
            trace.pass(pass, format!(r#"{{"pass":{pass}"#));
            trace.resolved(pass, Some(timed()));
        }
        trace.tick(start + Duration::from_millis(500));
        assert!(
            sink.lines().is_empty(),
            "flushed before a second had passed"
        );
        trace.tick(start + Duration::from_millis(1_000));
        assert_eq!(sink.lines().len(), 100);
    }

    /// Asking for a trace turns pacing on: a trace of nothing is no trace.
    #[test]
    fn the_trace_turns_pacing_on() {
        assert!(super::knob(false, true));
        assert!(super::knob(true, false));
        assert!(!super::knob(false, false));
    }

    /// **A measured pass's record goes through the counters**: it waits from
    /// `finish_at` for `gpu_resolved`; one whose GPU time came as it began,
    /// as a GPU that cannot time itself answers, goes with it; and the
    /// session's end writes what still waits as late.
    #[test]
    fn a_counted_pass_is_traced_with_its_gpu_time() {
        let sink = Shared::default();
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        *counters.trace.borrow_mut() = Some(Trace::new(Box::new(sink.clone()), start));
        assert!(
            counters
                .finish_at(start, start + ms(2), true, 1, 1)
                .is_none()
        );
        assert!(counters.gpu_resolved(1, timed()).is_none());
        // Pass 2, as `frame` numbers it, is being measured as its answer comes.
        assert!(counters.gpu_resolved(2, Gpu::Unsupported).is_none());
        assert!(
            counters
                .finish_at(start, start + ms(2), true, 1, 2)
                .is_none()
        );
        assert!(
            counters
                .finish_at(start, start + ms(2), true, 1, 3)
                .is_none()
        );
        assert!(counters.end(|| start + ms(9)).is_none());
        let written: Vec<(String, String)> = sink
            .lines()
            .iter()
            .map(|line| {
                (
                    field(line, "pass").to_owned(),
                    field(line, "gpu").to_owned(),
                )
            })
            .collect();
        let expected = [("1", "\"ok\""), ("2", "\"unsupported\""), ("3", "\"late\"")]
            .map(|(pass, gpu)| (pass.to_owned(), gpu.to_owned()));
        assert_eq!(written, expected);
    }

    /// **A record still waiting as the loop goes idle keeps waiting for its
    /// GPU time.** Nested, `winit.rs` reads that time only as the next pass
    /// begins, and its loop goes idle between a 60 Hz client's frames, so a
    /// record written at idle would read `late` on every such pass. Only the
    /// session's end writes what still waits as late.
    #[test]
    fn a_record_waiting_as_the_loop_goes_idle_keeps_waiting_for_its_gpu_time() {
        let sink = Shared::default();
        let counters = counters();
        counters.deadline.set(at_260());
        let start = Instant::now();
        *counters.trace.borrow_mut() = Some(Trace::new(Box::new(sink.clone()), start));
        assert!(
            counters
                .finish_at(start, start + ms(2), true, 1, 1)
                .is_none()
        );
        // The loop's next turn draws nothing, and reads no GPU time.
        assert!(counters.idle(|| start + ms(3)).is_none());
        // The next pass begins, and pass 1's time is read.
        assert!(counters.gpu_resolved(1, timed()).is_none());
        assert!(
            counters
                .finish_at(start + ms(17), start + ms(19), true, 1, 2)
                .is_none()
        );
        assert!(counters.idle(|| start + ms(20)).is_none());
        assert_eq!(sink.lines().len(), 1, "pass 2 went before its time came");
        assert!(counters.end(|| start + ms(21)).is_none());
        let written: Vec<(String, String)> = sink
            .lines()
            .iter()
            .map(|line| {
                (
                    field(line, "pass").to_owned(),
                    field(line, "gpu").to_owned(),
                )
            })
            .collect();
        let expected = [("1", "\"ok\""), ("2", "\"late\"")]
            .map(|(pass, gpu)| (pass.to_owned(), gpu.to_owned()));
        assert_eq!(written, expected);
    }

    /// **Through `frame`, `finish`, `flipped` and `summary`, as the
    /// backends call them**: a traced pass is stamped on CLOCK_MONOTONIC as it begins,
    /// which is the clock a script cuts its window by, and a flip takes its
    /// monitor's name only with a trace open (`Output::name` allocates).
    #[test]
    fn a_traced_pass_and_its_flip_through_the_backends_calls() {
        super::COUNTERS.with(|counters| {
            counters.asked.set(true);
            counters.on.set(true);
        });
        let landed = Flip { seq: 3, at: ms(4) };
        let queued = Queued {
            at: Duration::ZERO,
            after: None,
            pass: 1,
        };
        super::flipped(0, queued, landed, || {
            panic!("the name was taken with no trace open")
        });
        let sink = Shared::default();
        super::COUNTERS.with(|counters| {
            *counters.trace.borrow_mut() = Some(Trace::new(Box::new(sink.clone()), Instant::now()));
        });
        let before = super::monotonic_now().as_nanos();
        let pass = super::frame();
        let serial = pass.serial();
        pass.finish(1);
        let after = super::monotonic_now().as_nanos();
        super::flipped(
            0,
            Queued {
                pass: serial,
                ..queued
            },
            landed,
            || "DP-1".to_owned(),
        );
        super::summary();
        super::COUNTERS.with(|counters| *counters.trace.borrow_mut() = None);
        let lines = sink.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(field(&lines[0], "flip"), serial.to_string());
        assert_eq!(field(&lines[0], "monitor"), "\"DP-1\"");
        assert_eq!(field(&lines[1], "pass"), serial.to_string());
        let stamped: u128 = field(&lines[1], "t_ns").parse().unwrap_or(0);
        assert!(
            (before..=after).contains(&stamped),
            "t_ns {stamped} is not between {before} and {after}"
        );
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
            mark: std::cell::Cell::new(None),
            phase: std::cell::Cell::new(Phase::Loose),
            spent: [const { std::cell::Cell::new(0) }; Phase::COUNT],
            scenes: std::cell::Cell::new(0),
            animating: std::cell::Cell::new(0),
            rendered: std::cell::Cell::new(0),
            built: std::cell::Cell::new(0),
            rebound: std::cell::Cell::new(0),
            drew: std::cell::Cell::new(0),
            deadline: std::cell::Cell::new(Duration::ZERO),
            monitor: std::cell::RefCell::new(String::new()),
            since: std::cell::Cell::new(None),
            frames: std::cell::Cell::new(0),
            missed: std::cell::Cell::new(0),
            worst: std::cell::RefCell::new(None),
            reported: std::cell::Cell::new(None),
            all_passes: std::cell::Cell::new(0),
            all_missed: std::cell::Cell::new(0),
            late: std::cell::Cell::new(0),
            all_late: std::cell::Cell::new(0),
            parked: std::cell::RefCell::new(None),
            parked_at: std::cell::Cell::new(0),
            captures: std::cell::Cell::new(0),
            early: std::cell::Cell::new(None),
            clocks: std::cell::Cell::new(None),
            labels: std::cell::RefCell::new(Vec::new()),
            free_ids: std::cell::RefCell::new(Vec::new()),
            scene_spent: [const { std::cell::Cell::new((0, 0)) }; super::SCENE_SLOTS],
            scene_other: std::cell::Cell::new(0),
            trace: std::cell::RefCell::new(None),
            t_ns: std::cell::Cell::new(0),
            panes_seen: std::cell::Cell::new(0),
        }
    }
}
