#![expect(
    unsafe_code,
    reason = "dlopen of libnvidia-ml and calls through its C ABI"
)]

//! What the GPU's clocks are doing, beside what it spent.
//!
//! GL cannot say, and NVIDIA runs a desktop at P3 to P5 with its memory as
//! low as 810 MHz, so the same pass costs up to eight times more than at full
//! clocks. One thread, only while pacing is on, asks NVML (or, on an Intel
//! card, sysfs) four times a second and leaves the answer in atomics the
//! render thread reads for free. Logged and never acted on.

use std::{
    ffi::{CStr, c_char, c_int, c_uint, c_void},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

/// The clocks at the last sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Clocks {
    pub(crate) gpu_mhz: u32,
    /// 0 where the card does not say (i915).
    pub(crate) mem_mhz: u32,
    /// NVIDIA's performance state, 0 (full) to 15; `None` where unknown.
    pub(crate) pstate: Option<u8>,
}

/// Where the clocks come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Nvml,
    I915,
    None,
}

impl Source {
    /// As the PACING line and the trace spell it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Nvml => "nvml",
            Self::I915 => "i915",
            Self::None => "none",
        }
    }

    /// As `SOURCE` holds it: `pacing::tests::a_pass_takes_its_clocks_from_the_sampler`.
    const fn code(self) -> u32 {
        match self {
            Self::Nvml => 1,
            Self::I915 => 2,
            Self::None => 0,
        }
    }
}

const NVML: &CStr = c"libnvidia-ml.so.1";
const EVERY: Duration = Duration::from_millis(250);
const NVML_SUCCESS: c_int = 0;
const NVML_CLOCK_GRAPHICS: c_uint = 0;
const NVML_CLOCK_MEM: c_uint = 2;
/// Not a P-state, so read as unknown.
const UNKNOWN: u32 = 255;

static SOURCE: AtomicU32 = AtomicU32::new(0);
static GPU_MHZ: AtomicU32 = AtomicU32::new(0);
static MEM_MHZ: AtomicU32 = AtomicU32::new(0);
static PSTATE: AtomicU32 = AtomicU32::new(UNKNOWN);

/// Start sampling the clocks of the card behind DRM node `major:minor`.
/// Once, from the backend, only when pacing is on; `SOLIUM_PACING_CLOCKS=off`
/// keeps the thread from starting, for the run that checks the sampler does
/// not itself keep the GPU out of its low states.
pub(crate) fn start(major: u32, minor: u32) -> Source {
    if std::env::var("SOLIUM_PACING_CLOCKS").is_ok_and(|value| value.trim() == "off") {
        tracing::info!(
            clocks = "off",
            "SOLIUM_PACING_CLOCKS=off: the GPU's clocks are not sampled"
        );
        return Source::None;
    }
    let (tell, told) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("solium-clocks".to_owned())
        .spawn(move || sample(&probe(NVML, Path::new("/sys"), major, minor), &tell));
    if spawned.is_err() {
        return Source::None;
    }
    let source = told
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or(Source::None);
    tracing::info!(clocks = source.name(), "GPU clocks for pacing");
    source
}

/// Where the clocks come from, once `start` has answered.
pub(crate) fn source() -> Source {
    match SOURCE.load(Ordering::Relaxed) {
        1 => Source::Nvml,
        2 => Source::I915,
        _ => Source::None,
    }
}

/// The last sample, or `None` when nothing samples.
pub(crate) fn latest() -> Option<Clocks> {
    (source() != Source::None).then(|| Clocks {
        gpu_mhz: GPU_MHZ.load(Ordering::Relaxed),
        mem_mhz: MEM_MHZ.load(Ordering::Relaxed),
        pstate: pstate(PSTATE.load(Ordering::Relaxed)),
    })
}

/// Leave `clocks` from `source` where the sampler leaves them, for
/// `pacing::tests::a_pass_takes_its_clocks_from_the_sampler`, the one test
/// that writes them.
#[cfg(test)]
pub(crate) fn sampled(source: Source, clocks: Clocks) {
    GPU_MHZ.store(clocks.gpu_mhz, Ordering::Relaxed);
    MEM_MHZ.store(clocks.mem_mhz, Ordering::Relaxed);
    PSTATE.store(clocks.pstate.map_or(UNKNOWN, u32::from), Ordering::Relaxed);
    SOURCE.store(source.code(), Ordering::Relaxed);
}

/// NVML's P-state, 0 to 15, or `None`. `tests::nvml_pstates_map_and_unknown_is_none`.
fn pstate(raw: u32) -> Option<u8> {
    u8::try_from(raw).ok().filter(|state| *state <= 15)
}

/// The PCI address of the card behind a DRM node, as NVML names it.
/// `tests::a_drm_node_is_found_by_its_pci_address`.
fn pci_address(sysfs: &Path, major: u32, minor: u32) -> Option<String> {
    let device =
        std::fs::canonicalize(sysfs.join(format!("dev/char/{major}:{minor}/device"))).ok()?;
    Some(device.file_name()?.to_str()?.to_owned())
}

/// The actual frequency of the Intel card whose PCI directory is `device`.
/// `tests::an_intel_card_reports_its_actual_frequency`.
fn i915_mhz(device: &Path) -> Option<u32> {
    let cards = std::fs::read_dir(device.join("drm")).ok()?;
    cards.flatten().find_map(|card| {
        let name = card.file_name();
        if !name.to_string_lossy().starts_with("card") {
            return None;
        }
        std::fs::read_to_string(card.path().join("gt_act_freq_mhz"))
            .ok()?
            .trim()
            .parse()
            .ok()
    })
}

/// A dlopened library, closed when dropped.
#[derive(Debug)]
struct Library(*mut c_void);

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: a handle `dlopen` returned, closed once.
        unsafe { libc::dlclose(self.0) };
    }
}

impl Library {
    fn open(name: &CStr) -> Option<Self> {
        // SAFETY: a NUL-terminated name; the flags are dlopen's own.
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        // Not `then_some(Self(handle))`: that builds the `Library` before the
        // check, and dropping it closes a null handle, which crashes.
        // `tests::no_nvml_means_no_clocks`.
        if handle.is_null() {
            None
        } else {
            Some(Self(handle))
        }
    }

    /// One symbol, as a function pointer of type `F`.
    ///
    /// # Safety
    /// `F` must be the symbol's real signature.
    unsafe fn get<F: Copy>(&self, symbol: &CStr) -> Option<F> {
        // SAFETY: the handle is open; the caller vouches for `F`.
        unsafe {
            let found = libc::dlsym(self.0, symbol.as_ptr());
            (!found.is_null()).then(|| std::mem::transmute_copy::<*mut c_void, F>(&found))
        }
    }
}

type Device = *mut c_void;

/// NVML, opened, initialised and pointed at one card.
#[derive(Debug)]
struct Nvml {
    _library: Library,
    device: Device,
    clock: unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_int,
    pstate: unsafe extern "C" fn(Device, *mut c_uint) -> c_int,
}

/// What `start`'s thread found.
#[derive(Debug)]
enum Probe {
    Nvml(Nvml),
    I915(PathBuf),
    None,
}

/// Find a way to read the clocks of the card behind `major:minor`.
/// `tests::no_nvml_means_no_clocks`.
fn probe(soname: &CStr, sysfs: &Path, major: u32, minor: u32) -> Probe {
    let device = sysfs.join(format!("dev/char/{major}:{minor}/device"));
    if let Some(nvml) = pci_address(sysfs, major, minor).and_then(|pci| nvml(soname, &pci)) {
        return Probe::Nvml(nvml);
    }
    if i915_mhz(&device).is_some() {
        return Probe::I915(device);
    }
    Probe::None
}

/// NVML for the card at `pci`, or `None` with no driver library, or with
/// one that does not drive the card.
fn nvml(soname: &CStr, pci: &str) -> Option<Nvml> {
    let address = std::ffi::CString::new(pci).ok()?;
    let library = Library::open(soname)?;
    // SAFETY: the signatures are NVML's (`nvml.h`), versions `_v2` where NVML
    // has one.
    unsafe {
        let entry = Entry {
            init: library.get(c"nvmlInit_v2")?,
            shutdown: library.get(c"nvmlShutdown")?,
            by_pci: library.get(c"nvmlDeviceGetHandleByPciBusId_v2")?,
        };
        let clock = library.get(c"nvmlDeviceGetClockInfo")?;
        let pstate = library.get(c"nvmlDeviceGetPerformanceState")?;
        let device = device(entry, &address)?;
        Some(Nvml {
            _library: library,
            device,
            clock,
            pstate,
        })
    }
}

/// NVML's calls that start it, stop it and find a card.
#[derive(Clone, Copy, Debug)]
struct Entry {
    init: unsafe extern "C" fn() -> c_int,
    shutdown: unsafe extern "C" fn() -> c_int,
    by_pci: unsafe extern "C" fn(*const c_char, *mut Device) -> c_int,
}

/// Start NVML and find the card at `pci`. NVML started for a card it does
/// not drive is shut down again before its library can be closed under it
/// (a hybrid laptop's screens on the other GPU):
/// `tests::nvml_is_shut_down_when_the_card_is_not_its`.
///
/// # Safety
/// `entry` must be NVML's, or keep its contracts.
unsafe fn device(entry: Entry, pci: &CStr) -> Option<Device> {
    // SAFETY: the caller vouches for `entry`; the address is NUL-terminated
    // and the device is an out-pointer to a local.
    unsafe {
        if (entry.init)() != NVML_SUCCESS {
            return None;
        }
        let mut device: Device = std::ptr::null_mut();
        if (entry.by_pci)(pci.as_ptr(), &raw mut device) != NVML_SUCCESS {
            let _ = (entry.shutdown)();
            return None;
        }
        Some(device)
    }
}

/// The sampler: say what was found, then sample it for the life of the
/// process. Runs on its own thread, so no NVML call lands in a pass.
fn sample(probe: &Probe, tell: &std::sync::mpsc::Sender<Source>) {
    let source = match probe {
        Probe::Nvml(_) => Source::Nvml,
        Probe::I915(_) => Source::I915,
        Probe::None => Source::None,
    };
    SOURCE.store(source.code(), Ordering::Relaxed);
    let _ = tell.send(source);
    loop {
        match probe {
            Probe::None => return,
            Probe::Nvml(nvml) => {
                let (mut graphics, mut memory, mut state) = (0, 0, UNKNOWN);
                // SAFETY: a device handle NVML gave, and out-pointers to locals.
                unsafe {
                    if (nvml.clock)(nvml.device, NVML_CLOCK_GRAPHICS, &raw mut graphics)
                        != NVML_SUCCESS
                    {
                        graphics = 0;
                    }
                    if (nvml.clock)(nvml.device, NVML_CLOCK_MEM, &raw mut memory) != NVML_SUCCESS {
                        memory = 0;
                    }
                    if (nvml.pstate)(nvml.device, &raw mut state) != NVML_SUCCESS {
                        state = UNKNOWN;
                    }
                }
                GPU_MHZ.store(graphics, Ordering::Relaxed);
                MEM_MHZ.store(memory, Ordering::Relaxed);
                PSTATE.store(state, Ordering::Relaxed);
            }
            Probe::I915(device) => {
                GPU_MHZ.store(i915_mhz(device).unwrap_or(0), Ordering::Relaxed);
            }
        }
        std::thread::sleep(EVERY);
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use std::{
        ffi::{c_char, c_int},
        sync::atomic::{AtomicU32, Ordering},
    };

    use super::{Device, Entry, Probe, device, i915_mhz, pci_address, probe, pstate};

    /// A throwaway sysfs, under the test's own name.
    fn sysfs(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("solium-clocks-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a temporary sysfs");
        root
    }

    /// A render node is matched to its card by its PCI address, which is what
    /// NVML is asked for: `/sys/dev/char/226:128/device` names the card.
    #[test]
    fn a_drm_node_is_found_by_its_pci_address() {
        let root = sysfs("pci");
        let card = root.join("devices/pci0000:00/0000:08:00.0");
        std::fs::create_dir_all(&card).expect("the card's directory");
        let node = root.join("dev/char/226:128");
        std::fs::create_dir_all(&node).expect("the node's directory");
        std::os::unix::fs::symlink(&card, node.join("device")).expect("the device link");
        assert_eq!(
            pci_address(&root, 226, 128).as_deref(),
            Some("0000:08:00.0")
        );
        assert_eq!(
            pci_address(&root, 226, 129),
            None,
            "another node is not this one"
        );
    }

    /// No NVML and no i915 file: no clocks, and no thread to sample nothing.
    /// The node names a card, so the missing library is what is asked.
    #[test]
    fn no_nvml_means_no_clocks() {
        let root = sysfs("none");
        let card = root.join("devices/pci0000:00/0000:08:00.0");
        std::fs::create_dir_all(card.join("drm/card1")).expect("the card, with no frequency");
        let node = root.join("dev/char/226:128");
        std::fs::create_dir_all(&node).expect("the node's directory");
        std::os::unix::fs::symlink(&card, node.join("device")).expect("the device link");
        let probed = probe(c"libsolium-test-no-such-library.so.1", &root, 226, 128);
        assert!(matches!(probed, Probe::None));
    }

    /// How many times the fake NVML below was shut down.
    static SHUT_DOWN: AtomicU32 = AtomicU32::new(0);

    extern "C" fn starts() -> c_int {
        0
    }

    /// `NVML_ERROR_UNKNOWN`.
    extern "C" fn does_not_start() -> c_int {
        999
    }

    extern "C" fn shut_down() -> c_int {
        SHUT_DOWN.fetch_add(1, Ordering::Relaxed);
        0
    }

    /// `NVML_ERROR_NOT_FOUND`: the card is not one NVML drives.
    extern "C" fn not_its_card(_: *const c_char, _: *mut Device) -> c_int {
        6
    }

    /// # Safety
    /// `found` must be writable, as `device` passes it.
    unsafe extern "C" fn its_card(_: *const c_char, found: *mut Device) -> c_int {
        // SAFETY: the caller's out-pointer.
        unsafe { found.write(std::ptr::dangling_mut()) };
        0
    }

    /// **NVML started for a card it does not drive is shut down again**,
    /// before its library is closed under it: a hybrid laptop with NVIDIA's
    /// driver installed and its screens on the other GPU. NVML that did not
    /// start is not shut down, and NVML that found the card is kept.
    #[test]
    fn nvml_is_shut_down_when_the_card_is_not_its() {
        let pci = c"0000:00:02.0";
        // SAFETY: the fakes keep NVML's signatures and contracts.
        unsafe {
            let refused = Entry {
                init: starts,
                shutdown: shut_down,
                by_pci: not_its_card,
            };
            assert_eq!(device(refused, pci), None);
            assert_eq!(
                SHUT_DOWN.load(Ordering::Relaxed),
                1,
                "started, and shut down again"
            );
            let unstarted = Entry {
                init: does_not_start,
                ..refused
            };
            assert_eq!(device(unstarted, pci), None);
            assert_eq!(
                SHUT_DOWN.load(Ordering::Relaxed),
                1,
                "never started, so not shut down"
            );
            let found = Entry {
                by_pci: its_card,
                ..refused
            };
            assert!(device(found, pci).is_some());
            assert_eq!(
                SHUT_DOWN.load(Ordering::Relaxed),
                1,
                "found its card, so kept"
            );
        }
    }

    /// NVML's P-states are 0 to 15; 32 is "unknown", and so is anything else.
    #[test]
    fn nvml_pstates_map_and_unknown_is_none() {
        for state in 0..=15_u32 {
            assert_eq!(pstate(state), u8::try_from(state).ok());
        }
        assert_eq!(pstate(32), None);
        assert_eq!(pstate(255), None);
    }

    /// An Intel card reports the frequency it is actually running at, under
    /// its `drm/card*` directory beside the render node's.
    #[test]
    fn an_intel_card_reports_its_actual_frequency() {
        let root = sysfs("i915");
        let device = root.join("device");
        std::fs::create_dir_all(device.join("drm/card1")).expect("the card");
        std::fs::create_dir_all(device.join("drm/renderD128")).expect("the node");
        std::fs::write(device.join("drm/card1/gt_act_freq_mhz"), "1100\n").expect("the frequency");
        assert_eq!(i915_mhz(&device), Some(1100));
        assert_eq!(i915_mhz(Path::new("/nonexistent-solium-test")), None);
    }
}
