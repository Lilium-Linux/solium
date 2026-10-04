#![expect(
    unsafe_code,
    reason = "GL_EXT_disjoint_timer_query's two entry points are loaded through \
              egl::get_proc_address and called through function pointers"
)]
#![expect(
    dead_code,
    reason = "nothing reads the GPU timer until Task 4 wires it into the backends"
)]

//! How long the GPU spent on a pass, read passes later and never waited for.
//!
//! The CPU half of a pass is `pacing.rs`'s. This is the other half: a GL
//! timestamp at each end of a region of GPU work — a capture, an output's draw
//! — whose results are read once they are *available*, which is a few passes
//! later. Reading one before then blocks the CPU until the GPU catches up,
//! which is the stall this exists to measure rather than cause:
//! `tests::a_pass_is_read_three_passes_later_and_never_waited_for`.
//!
//! Timestamps, not `GL_TIME_ELAPSED`: elapsed queries cannot nest, and the
//! ladder's per-effect rows (X2.7) will sit inside an output's draw.
//!
//! Smithay and std only, so `dev/wirecheck` includes this file by path and
//! proves it on a GPU (case 11b).

use smithay::backend::renderer::gles::{GlesFrame, GlesRenderer, ffi};

const TIMESTAMP_EXT: ffi::types::GLenum = 0x8E28;
const QUERY_COUNTER_BITS_EXT: ffi::types::GLenum = 0x8864;
const QUERY_RESULT_EXT: ffi::types::GLenum = 0x8866;
const QUERY_RESULT_AVAILABLE_EXT: ffi::types::GLenum = 0x8867;
const GPU_DISJOINT_EXT: ffi::types::GLenum = 0x8FBB;

/// Passes whose queries may be in flight at once. Six at 260 Hz is 23 ms of
/// GPU queue, which nothing healthy exceeds: a slot reused before its results
/// came is dropped as late (`tests::an_unavailable_result_is_dropped_and_counted_late`).
const DEPTH: usize = 6;
/// Regions one pass may time, two queries each.
/// `tests::more_regions_than_the_pool_are_counted_not_issued`.
const REGIONS: usize = 16;

/// What a region of GPU work is charged to. X2.7 adds a row per effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Region {
    /// A window drawn into a texture of its own: `gpu_prep_us`.
    Capture,
    /// One output's draw, by its index in the backend's list.
    Output(u8),
}

/// One pass's GPU time. `tests::regions_sum_per_pass_and_captures_are_split_out`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GpuSample {
    /// Every region, summed: the compositor's own GPU time, not the span from
    /// first start to last end, which would count waits on Qt's fences.
    pub(crate) total_ns: u64,
    pub(crate) captures_ns: u64,
    /// The first four outputs; a fifth is in the total only.
    pub(crate) outputs_ns: [u64; 4],
    pub(crate) regions: u16,
    pub(crate) refused: u16,
}

/// What a pass's GPU time came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gpu {
    Ok(GpuSample),
    /// No `GL_EXT_disjoint_timer_query` here.
    Unsupported,
    /// The slot was needed again before the results came.
    Late,
    /// The GPU's clock jumped while the queries ran.
    Disjoint,
}

/// A region opened, to be closed. `None` when nothing was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "a region is closed with `close` or `close_in`"]
pub(crate) struct Stamp(Option<u8>);

/// The four GL calls the ring makes, so it can be tested with no GPU.
pub(crate) trait Queries {
    /// `glQueryCounterEXT(id, GL_TIMESTAMP_EXT)`.
    fn stamp(&mut self, id: u32);
    /// Whether `id`'s result is in. Never blocks.
    fn available(&mut self, id: u32) -> bool;
    /// `id`'s result, in nanoseconds. Only after `available`.
    fn read(&mut self, id: u32) -> u64;
    /// GL's disjoint flag, which reading clears.
    fn disjoint(&mut self) -> bool;
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    pass: Option<u64>,
    /// Each region and whether its end was stamped.
    regions: [Option<(Region, bool)>; REGIONS],
    used: usize,
    refused: u16,
}

impl Slot {
    const EMPTY: Self = Self {
        pass: None,
        regions: [None; REGIONS],
        used: 0,
        refused: 0,
    };
}

/// The passes in flight, and what has resolved.
#[derive(Debug)]
pub(crate) struct Ring {
    /// Query names, made once; `None` without the extension.
    ids: Option<[[u32; REGIONS * 2]; DEPTH]>,
    slots: [Slot; DEPTH],
    /// The slot being recorded into, which `resolve` leaves alone.
    current: Option<usize>,
    /// Passes resolved and not yet taken.
    done: Vec<(u64, Gpu)>,
}

impl Ring {
    pub(crate) fn new(ids: [[u32; REGIONS * 2]; DEPTH]) -> Self {
        Self {
            ids: Some(ids),
            slots: [Slot::EMPTY; DEPTH],
            current: None,
            done: Vec::with_capacity(DEPTH * 2),
        }
    }

    /// A ring that issues nothing and answers `Unsupported` for every pass.
    /// `tests::without_the_extension_nothing_is_issued`.
    pub(crate) fn unsupported() -> Self {
        Self {
            ids: None,
            slots: [Slot::EMPTY; DEPTH],
            current: None,
            done: Vec::with_capacity(DEPTH * 2),
        }
    }

    /// Begin `pass`: resolve whatever is in, then take its slot, dropping as
    /// late a pass still in it.
    pub(crate) fn begin<Q: Queries + ?Sized>(&mut self, queries: &mut Q, pass: u64) {
        if self.ids.is_none() {
            self.done.push((pass, Gpu::Unsupported));
            return;
        }
        self.current = None;
        self.resolve(queries);
        let index = usize::try_from(pass % DEPTH as u64).unwrap_or(0);
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        if let Some(stale) = slot.pass {
            self.done.push((stale, Gpu::Late));
        }
        *slot = Slot {
            pass: Some(pass),
            ..Slot::EMPTY
        };
        self.current = Some(index);
    }

    /// Stamp the start of a region of this pass.
    pub(crate) fn open<Q: Queries + ?Sized>(&mut self, queries: &mut Q, region: Region) -> Stamp {
        let (Some(ids), Some(index)) = (self.ids.as_ref(), self.current) else {
            return Stamp(None);
        };
        let Some(slot) = self.slots.get_mut(index) else {
            return Stamp(None);
        };
        if slot.used >= REGIONS {
            slot.refused = slot.refused.saturating_add(1);
            return Stamp(None);
        }
        let at = slot.used;
        let (Some(id), Some(entry)) = (
            ids.get(index).and_then(|ids| ids.get(at * 2)),
            slot.regions.get_mut(at),
        ) else {
            return Stamp(None);
        };
        *entry = Some((region, false));
        slot.used += 1;
        queries.stamp(*id);
        Stamp(u8::try_from(at).ok())
    }

    /// Stamp the end of a region opened in this pass.
    pub(crate) fn close<Q: Queries + ?Sized>(&mut self, queries: &mut Q, stamp: Stamp) {
        let (Some(ids), Some(index), Some(at)) = (self.ids.as_ref(), self.current, stamp.0) else {
            return;
        };
        let at = usize::from(at);
        let Some(Some((_, closed))) = self
            .slots
            .get_mut(index)
            .and_then(|slot| slot.regions.get_mut(at))
        else {
            return;
        };
        if *closed {
            return;
        }
        if let Some(id) = ids.get(index).and_then(|ids| ids.get(at * 2 + 1)) {
            *closed = true;
            queries.stamp(*id);
        }
    }

    /// The pass being recorded is over too: resolve everything in now.
    pub(crate) fn idle<Q: Queries + ?Sized>(&mut self, queries: &mut Q) {
        self.current = None;
        self.resolve(queries);
    }

    /// Every pass whose results are all in, read now. Never blocks.
    pub(crate) fn resolve<Q: Queries + ?Sized>(&mut self, queries: &mut Q) {
        let Self {
            ids,
            slots,
            current,
            done,
        } = self;
        let Some(ids) = ids.as_ref() else {
            return;
        };
        let mut disjoint: Option<bool> = None;
        for (index, (slot, ids)) in slots.iter_mut().zip(ids.iter()).enumerate() {
            if Some(index) == *current {
                continue;
            }
            let Some(pass) = slot.pass else {
                continue;
            };
            let id = |at: usize| ids.get(at).copied().unwrap_or(0);
            let ready = slot
                .regions
                .iter()
                .take(slot.used)
                .enumerate()
                .all(|(at, region)| match region {
                    Some((_, true)) => {
                        queries.available(id(at * 2)) && queries.available(id(at * 2 + 1))
                    }
                    Some((_, false)) => queries.available(id(at * 2)),
                    None => true,
                });
            if !ready {
                continue;
            }
            slot.pass = None;
            if *disjoint.get_or_insert_with(|| queries.disjoint()) {
                done.push((pass, Gpu::Disjoint));
                continue;
            }
            let mut sample = GpuSample {
                regions: u16::try_from(slot.used).unwrap_or(u16::MAX),
                refused: slot.refused,
                ..GpuSample::default()
            };
            for (at, region) in slot.regions.iter().take(slot.used).enumerate() {
                let Some((region, true)) = region else {
                    continue;
                };
                let (start, end) = (queries.read(id(at * 2)), queries.read(id(at * 2 + 1)));
                // `tests::an_end_before_its_start_is_discarded`.
                let Some(spent) = end.checked_sub(start) else {
                    continue;
                };
                sample.total_ns = sample.total_ns.saturating_add(spent);
                match region {
                    Region::Capture => {
                        sample.captures_ns = sample.captures_ns.saturating_add(spent);
                    }
                    Region::Output(output) => {
                        if let Some(sum) = sample.outputs_ns.get_mut(usize::from(*output)) {
                            *sum = sum.saturating_add(spent);
                        }
                    }
                }
            }
            done.push((pass, Gpu::Ok(sample)));
        }
    }
}

/// The two entry points GLES does not have.
#[derive(Clone, Copy)]
struct Ext {
    counter: unsafe extern "system" fn(ffi::types::GLuint, ffi::types::GLenum),
    result: unsafe extern "system" fn(ffi::types::GLuint, ffi::types::GLenum, *mut u64),
}

impl std::fmt::Debug for Ext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ext")
    }
}

/// The real GL behind [`Queries`], borrowed for one call.
struct Gl<'a> {
    gl: &'a ffi::Gles2,
    ext: Ext,
}

impl std::fmt::Debug for Gl<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gl")
    }
}

impl Queries for Gl<'_> {
    fn stamp(&mut self, id: u32) {
        // SAFETY: the context is current inside `with_context`, and `counter`
        // was loaded from it and checked non-null.
        unsafe { (self.ext.counter)(id, TIMESTAMP_EXT) }
    }
    fn available(&mut self, id: u32) -> bool {
        let mut ready = 0;
        // SAFETY: as above; `id` is a query name this context made.
        unsafe {
            self.gl
                .GetQueryObjectuiv(id, QUERY_RESULT_AVAILABLE_EXT, &raw mut ready);
        }
        ready != 0
    }
    fn read(&mut self, id: u32) -> u64 {
        let mut nanos = 0_u64;
        // SAFETY: as above, and only after `available` said yes.
        unsafe { (self.ext.result)(id, QUERY_RESULT_EXT, &raw mut nanos) };
        nanos
    }
    fn disjoint(&mut self) -> bool {
        let mut flag = 0;
        // SAFETY: as above.
        unsafe { self.gl.GetIntegerv(GPU_DISJOINT_EXT, &raw mut flag) };
        flag != 0
    }
}

/// Stands in when there is no extension; never called, because an
/// unsupported ring issues nothing.
#[derive(Debug)]
struct Never;

impl Queries for Never {
    fn stamp(&mut self, _id: u32) {}
    fn available(&mut self, _id: u32) -> bool {
        false
    }
    fn read(&mut self, _id: u32) -> u64 {
        0
    }
    fn disjoint(&mut self) -> bool {
        false
    }
}

/// Find the extension and make the query names, or `None`.
///
/// # Safety
/// The context must be current, which `with_context` guarantees.
unsafe fn load(gl: &ffi::Gles2) -> Option<(Ext, [[u32; REGIONS * 2]; DEPTH])> {
    // SAFETY: the caller's contract; `GetString` returns a static string.
    let listed = unsafe {
        let names = gl.GetString(ffi::EXTENSIONS);
        if names.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(names.cast())
            .to_string_lossy()
            .into_owned()
    };
    if !listed
        .split(' ')
        .any(|name| name == "GL_EXT_disjoint_timer_query")
    {
        return None;
    }
    // SAFETY: the context is current; get_proc_address "does not guarantee an
    // extension is actually supported", which is why the list is read first.
    let (counter, result) = unsafe {
        (
            smithay::backend::egl::get_proc_address("glQueryCounterEXT"),
            smithay::backend::egl::get_proc_address("glGetQueryObjectui64vEXT"),
        )
    };
    if counter.is_null() || result.is_null() {
        return None;
    }
    let mut bits = 0;
    // SAFETY: the context is current; a counter of no bits is no counter.
    unsafe { gl.GetQueryiv(TIMESTAMP_EXT, QUERY_COUNTER_BITS_EXT, &raw mut bits) };
    if bits <= 0 {
        return None;
    }
    // SAFETY: both pointers are non-null entry points of the listed
    // extension, with the signatures the extension specifies.
    let ext = unsafe {
        Ext {
            counter: std::mem::transmute::<
                *const std::ffi::c_void,
                unsafe extern "system" fn(ffi::types::GLuint, ffi::types::GLenum),
            >(counter),
            result: std::mem::transmute::<
                *const std::ffi::c_void,
                unsafe extern "system" fn(ffi::types::GLuint, ffi::types::GLenum, *mut u64),
            >(result),
        }
    };
    let mut ids = [[0_u32; REGIONS * 2]; DEPTH];
    for slot in &mut ids {
        // SAFETY: the context is current and `slot` has room for the count.
        unsafe { gl.GenQueries(i32::try_from(slot.len()).unwrap_or(0), slot.as_mut_ptr()) };
    }
    Some((ext, ids))
}

/// One renderer's GPU timer. Lives on `Solium` and only while pacing is on
/// (Ruling 5); each call is one `with_context`.
#[derive(Debug)]
pub(crate) struct Timer {
    ring: Ring,
    ext: Option<Ext>,
}

impl Timer {
    pub(crate) fn new(renderer: &mut GlesRenderer) -> Self {
        // SAFETY: `with_context` makes the renderer's context current.
        let loaded = renderer
            .with_context(|gl| unsafe { load(gl) })
            .ok()
            .flatten();
        match loaded {
            Some((ext, ids)) => Self {
                ring: Ring::new(ids),
                ext: Some(ext),
            },
            None => Self {
                ring: Ring::unsupported(),
                ext: None,
            },
        }
    }

    pub(crate) fn supported(&self) -> bool {
        self.ext.is_some()
    }

    pub(crate) fn begin_pass(&mut self, renderer: &mut GlesRenderer, pass: u64) {
        let Self { ring, ext } = self;
        match *ext {
            None => ring.begin(&mut Never, pass),
            Some(ext) => {
                let _ = renderer.with_context(|gl| ring.begin(&mut Gl { gl, ext }, pass));
            }
        }
    }

    /// Open a region outside a frame. Not between a nested backend's
    /// `render_output` and its `submit`: `with_context` unbinds the window's
    /// surface there (`render-pacing.md` §0 item 4).
    pub(crate) fn open(&mut self, renderer: &mut GlesRenderer, region: Region) -> Stamp {
        let Self { ring, ext } = self;
        let Some(ext) = *ext else {
            return Stamp(None);
        };
        renderer
            .with_context(|gl| ring.open(&mut Gl { gl, ext }, region))
            .unwrap_or(Stamp(None))
    }

    /// Open a region inside a frame, which makes nothing current.
    pub(crate) fn open_in(&mut self, frame: &mut GlesFrame<'_, '_>, region: Region) -> Stamp {
        let Self { ring, ext } = self;
        let Some(ext) = *ext else {
            return Stamp(None);
        };
        frame
            .with_context(|gl| ring.open(&mut Gl { gl, ext }, region))
            .unwrap_or(Stamp(None))
    }

    pub(crate) fn close(&mut self, renderer: &mut GlesRenderer, stamp: Stamp) {
        let Self { ring, ext } = self;
        let Some(ext) = *ext else {
            return;
        };
        let _ = renderer.with_context(|gl| ring.close(&mut Gl { gl, ext }, stamp));
    }

    pub(crate) fn close_in(&mut self, frame: &mut GlesFrame<'_, '_>, stamp: Stamp) {
        let Self { ring, ext } = self;
        let Some(ext) = *ext else {
            return;
        };
        let _ = frame.with_context(|gl| ring.close(&mut Gl { gl, ext }, stamp));
    }

    /// Nothing more is drawn for now: resolve everything that is in.
    pub(crate) fn idle(&mut self, renderer: &mut GlesRenderer) {
        let Self { ring, ext } = self;
        let Some(ext) = *ext else {
            return;
        };
        let _ = renderer.with_context(|gl| ring.idle(&mut Gl { gl, ext }));
    }

    /// The passes resolved since the last call.
    pub(crate) fn take_resolved(&mut self) -> std::vec::Drain<'_, (u64, Gpu)> {
        self.ring.done.drain(..)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{DEPTH, Gpu, GpuSample, Queries, REGIONS, Region, Ring};

    /// A GPU in a test: a stamp records the pass it was issued in and the next
    /// scripted value; its result is available `lag` passes later.
    #[derive(Debug, Default)]
    struct Fake {
        now: u64,
        lag: u64,
        values: Vec<u64>,
        stamped: BTreeMap<u32, (u64, u64)>,
        calls: u32,
        early_reads: u32,
        disjoint: bool,
        /// Passes more before an end's result is in than its start's, as on
        /// a GPU, where a region's end lands after its start. `ids()` gives
        /// starts odd names and ends even ones.
        end_lag: u64,
    }

    impl Fake {
        fn lagging(lag: u64) -> Self {
            Self {
                lag,
                ..Self::default()
            }
        }

        fn ready(&self, id: u32) -> bool {
            let end_lag = if id.is_multiple_of(2) {
                self.end_lag
            } else {
                0
            };
            self.stamped
                .get(&id)
                .is_some_and(|(at, _)| at + self.lag + end_lag <= self.now)
        }
    }

    impl Queries for Fake {
        fn stamp(&mut self, id: u32) {
            self.calls += 1;
            let value = if self.values.is_empty() {
                self.now * 1_000 + u64::from(id)
            } else {
                self.values.remove(0)
            };
            self.stamped.insert(id, (self.now, value));
        }
        fn available(&mut self, id: u32) -> bool {
            self.calls += 1;
            self.ready(id)
        }
        fn read(&mut self, id: u32) -> u64 {
            self.calls += 1;
            if !self.ready(id) {
                self.early_reads += 1;
            }
            self.stamped.get(&id).map_or(0, |(_, value)| *value)
        }
        fn disjoint(&mut self) -> bool {
            self.calls += 1;
            std::mem::take(&mut self.disjoint)
        }
    }

    /// Query names, distinct, as `glGenQueries` would hand them out.
    fn ids() -> [[u32; REGIONS * 2]; DEPTH] {
        let mut ids = [[0; REGIONS * 2]; DEPTH];
        let mut next = 1;
        for slot in &mut ids {
            for id in slot.iter_mut() {
                *id = next;
                next += 1;
            }
        }
        ids
    }

    /// One pass with one output region, the way a backend draws.
    fn pass(ring: &mut Ring, fake: &mut Fake, pass: u64) {
        fake.now = pass;
        ring.begin(fake, pass);
        let stamp = ring.open(fake, Region::Output(0));
        ring.close(fake, stamp);
    }

    /// **A pass is read when its results are in, and never before.** Reading an
    /// unavailable query blocks the CPU until the GPU catches up, which is the
    /// stall this whole module exists to measure rather than cause.
    #[test]
    fn a_pass_is_read_three_passes_later_and_never_waited_for() {
        let (mut ring, mut fake) = (Ring::new(ids()), Fake::lagging(3));
        for each in 1..=10 {
            pass(&mut ring, &mut fake, each);
        }
        let mut resolved: Vec<u64> = ring.done.iter().map(|(pass, _)| *pass).collect();
        resolved.sort_unstable();
        assert_eq!(
            resolved,
            (1..=7).collect::<Vec<_>>(),
            "passes 8 to 10 are not in yet"
        );
        assert!(ring.done.iter().all(|(_, gpu)| matches!(gpu, Gpu::Ok(_))));
        assert_eq!(
            fake.early_reads, 0,
            "a result was read before it was available"
        );
    }

    /// A slot about to be reused whose results never came is dropped as late,
    /// rather than waited for.
    #[test]
    fn an_unavailable_result_is_dropped_and_counted_late() {
        let (mut ring, mut fake) = (Ring::new(ids()), Fake::lagging(1_000));
        for each in 1..=(DEPTH as u64 + 2) {
            pass(&mut ring, &mut fake, each);
        }
        assert_eq!(ring.done, vec![(1, Gpu::Late), (2, Gpu::Late)]);
        assert_eq!(fake.early_reads, 0);
    }

    /// The regions of one pass add up, and captures are counted apart from
    /// outputs, which is `gpu_prep_us` against the rest.
    #[test]
    fn regions_sum_per_pass_and_captures_are_split_out() {
        let mut fake = Fake::lagging(0);
        fake.values = vec![1_000, 1_100, 1_200, 1_400, 1_500, 2_000];
        let mut ring = Ring::new(ids());
        fake.now = 1;
        ring.begin(&mut fake, 1);
        for region in [Region::Capture, Region::Capture, Region::Output(0)] {
            let stamp = ring.open(&mut fake, region);
            ring.close(&mut fake, stamp);
        }
        fake.now = 2;
        ring.begin(&mut fake, 2);
        let mut expected = GpuSample {
            total_ns: 800,
            captures_ns: 300,
            regions: 3,
            ..GpuSample::default()
        };
        expected.outputs_ns[0] = 500;
        assert_eq!(ring.done, vec![(1, Gpu::Ok(expected))]);
    }

    /// GL's disjoint flag means the clock jumped while those queries ran:
    /// everything resolved at that moment is thrown away, not reported.
    #[test]
    fn a_disjoint_flag_discards_what_was_pending() {
        let (mut ring, mut fake) = (Ring::new(ids()), Fake::lagging(0));
        pass(&mut ring, &mut fake, 1);
        pass(&mut ring, &mut fake, 2);
        fake.disjoint = true;
        fake.now = 3;
        ring.begin(&mut fake, 3);
        assert_eq!(
            ring.done,
            vec![(1, Gpu::Ok(ring_sample(&fake, 1))), (2, Gpu::Disjoint)]
        );
    }

    /// What pass 1 of `a_disjoint_flag_discards_what_was_pending` came to,
    /// resolved by pass 2's `begin` before the flag was raised.
    fn ring_sample(_fake: &Fake, _pass: u64) -> GpuSample {
        let mut sample = GpuSample {
            total_ns: 1,
            regions: 1,
            ..GpuSample::default()
        };
        sample.outputs_ns[0] = 1;
        sample
    }

    /// Without the extension nothing is issued at all, and every pass says so.
    #[test]
    fn without_the_extension_nothing_is_issued() {
        let (mut ring, mut fake) = (Ring::unsupported(), Fake::lagging(0));
        pass(&mut ring, &mut fake, 1);
        assert_eq!(fake.calls, 0, "a GL call was made with no extension");
        assert_eq!(ring.done, vec![(1, Gpu::Unsupported)]);
    }

    /// A pass with more regions than its slot holds counts the rest instead of
    /// issuing them: a query name past the slot is another slot's.
    #[test]
    fn more_regions_than_the_pool_are_counted_not_issued() {
        let (mut ring, mut fake) = (Ring::new(ids()), Fake::lagging(0));
        fake.now = 1;
        ring.begin(&mut fake, 1);
        for _ in 0..=REGIONS {
            let stamp = ring.open(&mut fake, Region::Capture);
            ring.close(&mut fake, stamp);
        }
        assert_eq!(
            fake.stamped.len(),
            REGIONS * 2,
            "one region too many was issued"
        );
        fake.now = 2;
        ring.begin(&mut fake, 2);
        assert!(
            matches!(ring.done.first(), Some((1, Gpu::Ok(sample))) if sample.regions == 16 && sample.refused == 1)
        );
    }

    /// A region is read once its end is in, not only its start: on a GPU the
    /// end lands later, and reading it early is the stall.
    #[test]
    fn a_region_waits_for_its_end_not_only_its_start() {
        let (mut ring, mut fake) = (Ring::new(ids()), Fake::lagging(1));
        fake.end_lag = 2;
        for each in 1..=5 {
            pass(&mut ring, &mut fake, each);
        }
        assert_eq!(
            fake.early_reads, 0,
            "an end was read before it was available"
        );
        let resolved: Vec<u64> = ring.done.iter().map(|(pass, _)| *pass).collect();
        assert_eq!(resolved, vec![1, 2], "an end is in three passes later");
    }

    /// An end that reads before its start is a wrapped or broken counter, and
    /// adds nothing rather than four billion seconds.
    #[test]
    fn an_end_before_its_start_is_discarded() {
        let mut fake = Fake::lagging(0);
        fake.values = vec![5_000, 4_000];
        let mut ring = Ring::new(ids());
        fake.now = 1;
        ring.begin(&mut fake, 1);
        let stamp = ring.open(&mut fake, Region::Output(1));
        ring.close(&mut fake, stamp);
        fake.now = 2;
        ring.begin(&mut fake, 2);
        assert!(matches!(ring.done.first(), Some((1, Gpu::Ok(sample))) if sample.total_ns == 0));
    }
}
