//! Qt Quick scenes, hosted in this process.
//!
//! The compositor's shell surfaces — the bar now, window decorations next —
//! are authored in QML and rendered by Qt's scene graph inside this process.
//! See `qml/host.cpp` for why in-process: a shell painting frames over a
//! protocol was tried and measured at ~15 fps, 39% CPU.
//!
//! Two paths, and only ever one of them per process, because Qt fixes its
//! scene graph backend inside `QGuiApplication`. The software rasteriser is the
//! default and draws into a `QImage` the compositor uploads — no QPA plugin
//! here will adopt a foreign EGL context, so a GL scene graph cannot simply be
//! handed Solium's own. The GPU path, behind `SOLIUM_QML_GPU`, works around
//! that from the other end: the compositor allocates the buffer through GBM and
//! Qt imports its dmabuf as a texture to draw into. See `start_on_gpu`.
//!
//! This module is the only unsafe surface in the compositor, and it is kept
//! deliberately narrow: a handle, a render call, some setters. Everything Qt is
//! behind the C ABI.

mod target;

use std::{
    ffi::{CStr, CString, c_char, c_double, c_int, c_longlong, c_uint, c_ulonglong},
    os::fd::{FromRawFd as _, OwnedFd},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use smithay::{
    backend::{
        allocator::gbm::GbmDevice,
        drm::{DrmDeviceFd, DrmNode, NodeType},
        udev,
    },
    utils::DeviceFd,
};

#[expect(
    unsafe_code,
    reason = "the Qt host is C++; this is the declaration of its C ABI"
)]
mod ffi {
    use super::{c_char, c_double, c_int, c_longlong, c_uint, c_ulonglong};

    #[repr(C)]
    pub(super) struct Scene {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        pub(super) fn solium_qml_start(import_path: *const c_char) -> c_int;
        pub(super) fn solium_qml_start_gpu(import_path: *const c_char) -> c_int;
        pub(super) fn solium_qml_scene_new_gpu(
            qml_path: *const c_char,
            width: c_int,
            height: c_int,
            dmabuf_fd: c_int,
            stride: c_int,
            modifier: c_ulonglong,
            fourcc: c_uint,
            initial_json: *const c_char,
        ) -> *mut Scene;
        pub(super) fn solium_qml_scene_render_gpu(scene: *mut Scene, fence_fd: *mut c_int)
        -> c_int;
        pub(super) fn solium_qml_set_windows(json: *const c_char);
        pub(super) fn solium_qml_clear_cache();
        pub(super) fn solium_qml_scene_new_with(
            qml_path: *const c_char,
            width: c_int,
            height: c_int,
            initial_json: *const c_char,
            error: *mut *const c_char,
        ) -> *mut Scene;
        pub(super) fn solium_qml_scene_free(scene: *mut Scene);
        pub(super) fn solium_qml_scene_resize(
            scene: *mut Scene,
            width: c_int,
            height: c_int,
            scale: f64,
        );
        pub(super) fn solium_qml_tick(elapsed_ms: c_longlong);
        pub(super) fn solium_qml_scene_render(scene: *mut Scene) -> c_int;
        pub(super) fn solium_qml_scene_pixels(scene: *const Scene, stride: *mut c_int)
        -> *const u8;
        pub(super) fn solium_qml_scene_take_string(
            scene: *mut Scene,
            name: *const c_char,
        ) -> *const c_char;
        pub(super) fn solium_qml_scene_set_string(
            scene: *mut Scene,
            name: *const c_char,
            value: *const c_char,
        );
        pub(super) fn solium_qml_scene_set_int(
            scene: *mut Scene,
            name: *const c_char,
            value: c_int,
        );
        pub(super) fn solium_qml_scene_get_int(scene: *mut Scene, name: *const c_char) -> c_int;
        pub(super) fn solium_qml_scene_set_bool(
            scene: *mut Scene,
            name: *const c_char,
            value: c_int,
        );
        pub(super) fn solium_qml_scene_get_bool(scene: *const Scene, name: *const c_char) -> c_int;
        pub(super) fn solium_qml_scene_dirty(scene: *const Scene) -> c_int;
        pub(super) fn solium_qml_scene_pointer(
            scene: *mut Scene,
            x: c_double,
            y: c_double,
            pressed: c_int,
        );
    }
}

/// Start Qt. Idempotent, and must happen on the thread that renders.
///
/// Every scene shares one engine and one import path, which is what makes the
/// design system a single object rather than a copy per surface — see
/// `qml/Solium/Theme.qml`.
#[expect(unsafe_code, reason = "calling into the Qt host")]
pub(crate) fn start() -> Result<()> {
    let path = import_path();
    let path = PathBuf::from(path);
    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| anyhow!("the QML import path contains a NUL byte"))?;

    // Which scene graph this process came up on, decided once. Qt fixes that
    // for the life of the process — there is no second chance and no way back
    // — so the whole decision, availability included, is made inside this
    // `OnceLock` and every later `start()` reads the answer rather than
    // re-asking the question.
    if crate::dev::qml_gpu() && *GPU.get_or_init(|| start_on_gpu(&path)) {
        return Ok(());
    }

    // SAFETY: `path` outlives the call, and the shim is idempotent.
    if unsafe { ffi::solium_qml_start(path.as_ptr()) } == 0 {
        return Err(anyhow!("could not start Qt"));
    }
    Ok(())
}

/// Whether Qt came up on the GPU. `None` until the first scene asks for one.
static GPU: OnceLock<bool> = OnceLock::new();

/// The scene the pre-flight renders, and how big.
///
/// Small on purpose: it is proving that a buffer can be allocated, imported,
/// drawn into and fenced, and none of that gets more true at a larger size.
const PREFLIGHT_SIDE: i32 = 64;

/// Bring Qt up on the GPU, and prove the round trip before trusting it.
///
/// Two hard constraints shape this, both measured rather than assumed.
///
/// The first is that `solium_qml_start_gpu` **cannot fail politely**. It picks
/// the QPA platform plugin, Qt loads that inside `QGuiApplication`'s
/// constructor, and Qt calls `qFatal` on a plugin it cannot bring up — SIGABRT,
/// exit 134, no return value to inspect. So everything that can be decided
/// before it is decided before it, and a `false` from this function is always a
/// decision taken while the software path was still reachable.
///
/// The second is that a `1` back from it is not evidence the path *works*: it
/// says Qt came up on an RHI, and every remaining thing the GPU path depends on
/// — the render control initialising, the dmabuf importing, the driver handing
/// back a fence — is per scene. By then Qt is committed and there is no
/// fallback left, so the honest thing is not to pretend one exists but to find
/// out immediately and say so, once, at startup. That is the pre-flight: real
/// QML, a real buffer from `target::allocate`, the real import and the real
/// fence, before anything on screen depends on any of it.
#[expect(unsafe_code, reason = "calling into the Qt host")]
fn start_on_gpu(import_path: &CStr) -> bool {
    let Some(node) = render_node() else {
        tracing::warn!(
            "SOLIUM_QML_GPU is set and no DRM render node could be found; using software"
        );
        return false;
    };
    if let Err(err) = point_qt_away_from_the_card(&node) {
        tracing::warn!(?err, "could not fence Qt off the card node; using software");
        return false;
    }
    // Said out loud because Smithay logs `unable to become drm master` from
    // inside the next call — `DrmDeviceFd` asks for it on any node it is handed
    // — and on a render node it can never be granted and is never needed. That
    // one warning is also the only visible symptom of Qt stealing master from
    // the compositor, which is what the KMS config above exists to prevent, and
    // a reader who finds it unexplained has to work out which of the two it is.
    tracing::info!(
        node = %node.display(),
        "opening the render node for QML; the drm master warning that follows is about it"
    );
    // Before Qt, not after: a device we cannot open is a reason to stay on the
    // software path, and after the next call that is no longer a choice.
    let gbm = match open_render_node(&node) {
        Ok(gbm) => gbm,
        Err(err) => {
            tracing::warn!(?err, node = %node.display(), "no GBM device; using software");
            return false;
        }
    };

    // SAFETY: `import_path` outlives the call. This is the point of no return:
    // it either sets the scene graph backend for the process or aborts it.
    if unsafe { ffi::solium_qml_start_gpu(import_path.as_ptr()) } != 1 {
        // Reachable only when Qt was already up on the software backend, since
        // everything else inside is either infallible or fatal. Nothing has
        // changed in that case, so the software path is still the right answer.
        tracing::warn!("Qt is already up on the software scene graph; staying there");
        return false;
    }

    match preflight(&gbm) {
        Ok(fenced) => {
            tracing::info!(
                node = %node.display(),
                fenced,
                "QML on the GPU: Qt rendered into a buffer we allocated"
            );
            // The other half of that sentence, and the reason this knob is off
            // by default. A GPU host cannot build software scenes — it is one
            // scene graph per process, and Qt picked this one — so every
            // surface that has not been moved onto `Scene::gpu` now fails to
            // load and draws nothing. The session runs; it is bare.
            tracing::warn!(
                "SOLIUM_QML_GPU: scenes that are not GPU scenes will not load, so anything \
                 still on the software path draws nothing until it is converted"
            );
        }
        // Loud, and not fatal. Qt's backend is fixed by now, so this cannot
        // fall back — but a compositor that quit here would take the session
        // with it for the sake of a knob that is off by default, and the reason
        // this line exists is so the failure is read here rather than guessed
        // at from a blank screen ten seconds later.
        Err(err) => tracing::error!(
            ?err,
            "the GPU path came up and does not work; scenes will not draw. \
             unset SOLIUM_QML_GPU to go back to software rendering"
        ),
    }
    true
}

/// Render one frame of real QML into a real dmabuf, and say whether it fenced.
///
/// `Ok(false)` is a pass, not a failure: the driver declining to export a fence
/// means the host waited on the CPU with `glFinish` instead, which is correct
/// and only costs a stall.
fn preflight(gbm: &GbmDevice<DrmDeviceFd>) -> Result<bool> {
    let target = target::allocate(gbm, PREFLIGHT_SIDE, PREFLIGHT_SIDE)
        .context("allocating a buffer to test the GPU path with")?;
    let qml = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/probe.qml"));
    let mut scene = Scene::gpu(&qml, PREFLIGHT_SIDE, PREFLIGHT_SIDE, target, None)?;
    // A scene is dirty the moment it is built, so this always renders. `None`
    // would mean Qt thought a scene it has never drawn was already up to date,
    // which is not a pass by another name.
    let fence = scene
        .render_gpu()?
        .ok_or_else(|| anyhow!("Qt reported a brand new scene as already up to date"))?;
    Ok(fence.is_some())
}

/// The render node of the GPU this seat boots on.
///
/// The *render* node, never the card: see `point_qt_away_from_the_card`. Found
/// through udev rather than taken from `tty::State::open_gpu`, which computes
/// the same thing at `tty.rs:1110-1113`, because scenes are built while the
/// scripts load and that happens *before* the GPU is opened — the ordering is
/// the whole reason the next function exists. Asking udev works in either
/// order, and on the nested backend, where `open_gpu` never runs at all.
fn render_node() -> Option<PathBuf> {
    let seat = std::env::var("XDG_SEAT").unwrap_or_else(|_| "seat0".to_owned());
    let card = udev::primary_gpu(&seat)
        .inspect_err(|err| tracing::warn!(?err, seat, "asking udev for the primary GPU"))
        .ok()
        .flatten()?;
    let node = DrmNode::from_path(&card)
        .inspect_err(|err| tracing::warn!(?err, card = %card.display(), "reading the DRM node"))
        .ok()?;
    node.dev_path_with_type(NodeType::Render)
}

/// Point Qt's eglfs at a render node, headless, so it cannot take DRM master.
///
/// eglfs does not *need* master and cannot take one by asking. The kernel gives
/// it away implicitly, to whoever opens the card node while nobody holds it —
/// and Qt starts here, which on the hardware backend is before `open_gpu`: the
/// scripts load first, and a `sol.surface` in a user's configuration builds a
/// scene, which starts Qt. Qt's fd becomes master, logind's `SetMaster` then
/// returns `EBUSY`, and the only thing said out loud about it is Smithay's
/// `unable to become drm master`, which `tty.rs` correctly documents as benign
/// noise for the ordinary case. A black screen on a TTY with nothing to read.
///
/// Both JSON keys are required, and that is measured: `device` on its own fails
/// the plugin with `drmModeGetResources failed (Permission denied)`, and
/// `headless` on its own still opens the card. With both, Qt opens the card
/// zero times and issues zero ioctls against it, and still renders QML into the
/// dmabuf. `QT_QPA_EGLFS_DEVICE` does not exist in this Qt build; the config
/// file is the only way in.
fn point_qt_away_from_the_card(node: &Path) -> Result<()> {
    let node = node
        .to_str()
        .ok_or_else(|| anyhow!("the render node's path is not UTF-8"))?;
    // Hand-written JSON, so a path that needed escaping would produce a config
    // Qt reads as malformed and reports as "no device" — which looks exactly
    // like the machine having no GPU. Device nodes are named out of a very
    // small alphabet, so this is a should-never-happen guard.
    if node.contains(['"', '\\']) {
        return Err(anyhow!(
            "the render node's path needs JSON escaping: {node}"
        ));
    }
    let config = runtime_dir().join("solium-eglfs-kms.json");
    std::fs::write(
        &config,
        format!("{{ \"device\": \"{node}\", \"headless\": \"64x64\" }}\n"),
    )
    .with_context(|| format!("writing {}", config.display()))?;

    // SAFETY: `set_var` is unsound only against a concurrent reader of the
    // environment. This runs from `qml::start`, on the thread that renders,
    // before Qt exists — Qt reads all three inside `QGuiApplication`'s
    // constructor, which is the call after this one — and nothing else in the
    // compositor reads the environment off the main thread.
    #[expect(unsafe_code, reason = "std::env::set_var is unsafe in edition 2024")]
    unsafe {
        std::env::set_var("QT_QPA_EGLFS_KMS_CONFIG", &config);
        // No input and no event reader: the compositor owns the devices, and a
        // second reader of them inside Qt is a second consumer of every event.
        std::env::set_var("QT_QPA_EGLFS_DISABLE_INPUT", "1");
        std::env::set_var("QT_QPA_EGLFS_KMS_NO_EVENT_READER_THREAD", "1");
    }
    tracing::debug!(config = %config.display(), node, "pointed Qt's eglfs at the render node");
    Ok(())
}

/// A GBM device on the render node, for the buffers Qt draws into.
///
/// Opened directly rather than through the session, which is the one thing a
/// render node is for: it grants no master, needs no seat, and on the hardware
/// backend there is no session to open it through at this point anyway — the
/// scripts, and so the first scene, run before `open_gpu`.
fn open_render_node(node: &Path) -> Result<GbmDevice<DrmDeviceFd>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)
        .with_context(|| format!("opening {}", node.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file)));
    GbmDevice::new(fd).context("creating the GBM device")
}

/// Somewhere to write a file Qt has to be able to read back by path.
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Forget compiled QML, so the next scene is read from disk.
#[expect(unsafe_code, reason = "calling into the Qt host")]
pub(crate) fn clear_cache() {
    // SAFETY: the host checks that it has an engine.
    unsafe { ffi::solium_qml_clear_cache() }
}

/// Hand the shell the compositor's window list.
///
/// The shell reads `ToplevelManager.toplevels` and `Hyprland.activeToplevel`;
/// both are answered from this. JSON because the boundary is a C string, and a
/// window list is small enough that its cost is not worth a bespoke encoding.
#[expect(unsafe_code, reason = "calling into the Qt host")]
pub(crate) fn set_windows(json: &str) {
    let Ok(json) = CString::new(json) else {
        return;
    };
    // SAFETY: the string outlives the call, which copies what it needs.
    unsafe { ffi::solium_qml_set_windows(json.as_ptr()) }
}

/// Where QML modules are found, `Solium` among them.
///
/// Colon-separated, like a `PATH`, because shell code brought in from
/// elsewhere needs its own modules and a compatibility layer on the search
/// path beside the compositor's. Overridable so a whole design system can be
/// swapped without rebuilding, which is most of the point of it being QML.
fn import_path() -> std::ffi::OsString {
    if let Some(path) = std::env::var_os("SOLIUM_QML_PATH") {
        return path;
    }
    let own = concat!(env!("CARGO_MANIFEST_DIR"), "/qml");
    let shim = concat!(env!("CARGO_MANIFEST_DIR"), "/qml/compat");
    // The user's directory first, for the same reason the Lua search path puts
    // it first: dropping `Solium/Theme.qml` into ~/.config/solium/qml should
    // restyle every frame and every surface, without copying the rest.
    match user_qml_dir() {
        Some(user) => std::ffi::OsString::from(format!("{}:{own}:{shim}", user.display())),
        None => std::ffi::OsString::from(format!("{own}:{shim}")),
    }
}

/// `~/.config/solium/qml`, if it exists.
pub(crate) fn user_qml_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })?;
    let path = home.join("solium").join("qml");
    path.is_dir().then_some(path)
}

/// Advance every animation in the process, once per compositor frame.
///
/// Separate from rendering on purpose. The clock has to keep running even for
/// scenes that look settled -- an animation that is not advanced never
/// changes, so it never asks to be drawn, so it never gets advanced again, and
/// a loop with a pause in it dies at its first pause. Ticking is cheap and
/// unconditional; rendering waits to be asked.
#[expect(unsafe_code, reason = "calling into the Qt host")]
pub(crate) fn tick(elapsed: Duration) {
    let millis = c_longlong::try_from(elapsed.as_millis()).unwrap_or(c_longlong::MAX);
    // SAFETY: the host is started before any scene exists, and this touches
    // only process-global state.
    unsafe { ffi::solium_qml_tick(millis) }
}

/// Matches `SOLIUM_QML_UNCHANGED` in `qml/host.h`.
const UNCHANGED: c_int = 2;

/// The result of a render: the scene's pixels, and whether they are new.
#[derive(Debug)]
pub(crate) struct Rendered<'a> {
    pub(crate) changed: bool,
    pub(crate) pixels: &'a [u8],
    pub(crate) stride: usize,
}

/// A live QML scene.
pub(crate) struct Scene {
    scene: *mut ffi::Scene,
    /// Device pixels — the size of the image the compositor uploads.
    size: (i32, i32),
    /// Device pixels per logical one.
    scale: f64,
    /// The buffer Qt draws into, on the GPU path; `None` on the software one.
    ///
    /// Held for the life of the scene even though nothing here reads it after
    /// the constructor. Qt is finished with it — EGL took its own reference
    /// during the import and the host keeps no fd — but the compositor
    /// re-imports the same dmabuf to sample what Qt drew, so it has to still
    /// exist. Nothing else owns it.
    target: Option<target::Target>,
}

// The scene is bound to the GL context it was created on, and that context
// belongs to the render thread. Not Send, deliberately: sending it elsewhere
// would put Qt's scene graph on a thread with no current context.
impl Scene {
    /// Load a QML file into a scene of the given size.
    pub(crate) fn new(qml_path: &Path, width: i32, height: i32) -> Result<Self> {
        Self::with_properties(qml_path, width, height, None)
    }

    /// Build a scene, supplying properties it declares as required.
    ///
    /// Required properties must be given *at creation*: setting them after the
    /// fact is too late and the component never builds. `initial` is a JSON
    /// object.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn with_properties(
        qml_path: &Path,
        width: i32,
        height: i32,
        initial: Option<&str>,
    ) -> Result<Self> {
        let path = CString::new(qml_path.as_os_str().as_encoded_bytes())
            .map_err(|_| anyhow!("the QML path contains a NUL byte"))?;
        let initial = initial
            .map(|json| CString::new(json).map_err(|_| anyhow!("properties contain a NUL byte")))
            .transpose()?;
        let initial_ptr = initial
            .as_ref()
            .map_or(std::ptr::null(), |value| value.as_ptr());
        let mut error: *const c_char = std::ptr::null();

        // SAFETY: `path` and `initial` outlive the call; `error` is only read
        // when the call returns null, which is when the shim has set it.
        let scene = unsafe {
            ffi::solium_qml_scene_new_with(
                path.as_ptr(),
                width,
                height,
                initial_ptr,
                &raw mut error,
            )
        };

        if scene.is_null() {
            let reason = if error.is_null() {
                "unknown".to_owned()
            } else {
                // SAFETY: the shim sets this to a NUL-terminated string with
                // static or long-lived storage.
                unsafe { CStr::from_ptr(error) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(anyhow!("loading {}: {reason}", qml_path.display()));
        }

        Ok(Self {
            scene,
            size: (width, height),
            scale: 1.0,
            target: None,
        })
    }

    /// A scene that renders into `target` on the GPU.
    ///
    /// The `Target` is moved in rather than borrowed. Qt has no further use for
    /// it once this returns, but the buffer is what the compositor samples, and
    /// tying its life to the scene's is the only arrangement in which the thing
    /// being read cannot be freed while something is still drawing into it.
    ///
    /// Leaves *Qt's* GL context current on this thread: `QQuickRenderControl::
    /// initialize` makes it so and does not put anything back. See
    /// `render_gpu`.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn gpu(
        qml_path: &Path,
        width: i32,
        height: i32,
        target: target::Target,
        initial: Option<&str>,
    ) -> Result<Self> {
        // A GPU scene's pixel size is the buffer's, and it is fixed at
        // allocation: the texture is exactly as big as the dmabuf, and
        // `solium_qml_scene_resize` refuses a pixel-size change on one rather
        // than silently swapping the texture for a paint device and moving the
        // scene back onto the CPU. A caller that allocated one size and asked
        // for another would hear about it a frame later, from the host, in a
        // warning about resizing — nowhere near the mistake. So it is answered
        // here, where the two sizes are both in scope.
        if (target.width, target.height) != (width, height) {
            return Err(anyhow!(
                "a {width}x{height} scene cannot render into a {}x{} buffer",
                target.width,
                target.height
            ));
        }
        let (fd, stride, modifier, fourcc) = target.as_ffi()?;
        let path = CString::new(qml_path.as_os_str().as_encoded_bytes())
            .map_err(|_| anyhow!("the QML path contains a NUL byte"))?;
        let initial = initial
            .map(|json| CString::new(json).map_err(|_| anyhow!("properties contain a NUL byte")))
            .transpose()?;

        // SAFETY: every pointer outlives the call. The fd is borrowed for the
        // length of it and nothing more: the host neither keeps nor dups it,
        // because EGL takes its own reference on the buffer inside
        // `eglCreateImageKHR` — measured, by closing the fd immediately after
        // the import and rendering correctly anyway. `target` stays the only
        // owner, and it outlives this call by being moved into the scene below.
        let scene = unsafe {
            ffi::solium_qml_scene_new_gpu(
                path.as_ptr(),
                width,
                height,
                fd,
                stride,
                c_ulonglong::from(modifier),
                c_uint::from(fourcc),
                initial
                    .as_ref()
                    .map_or(std::ptr::null(), |value| value.as_ptr()),
            )
        };
        if scene.is_null() {
            // No error string to report, unlike the software constructor: every
            // way this fails is a property of the driver or of Qt rather than
            // of the QML, so the useful detail is an EGL or GL code and the
            // host has already written it to the warning log.
            return Err(anyhow!(
                "the GPU scene would not load: {}",
                qml_path.display()
            ));
        }
        Ok(Self {
            scene,
            size: (width, height),
            scale: 1.0,
            target: Some(target),
        })
    }

    /// Render, returning a fence that signals when Qt's work has landed.
    ///
    /// `Ok(None)` means the scene was already up to date. A `None` fence inside
    /// `Some` means the driver gave none and the host waited with `glFinish`
    /// instead, so the frame is already complete — that is a correct answer and
    /// not an error, it just costs a stall rather than a hand-off.
    ///
    /// Leaves *Qt's* GL context current on this thread. Whoever else holds a
    /// context here has to make it current again before the next GL call, or
    /// that call fails somewhere with nothing to do with this one.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn render_gpu(&mut self) -> Result<Option<Option<OwnedFd>>> {
        if self.target.is_none() {
            // The host would refuse this too, with a warning. Refusing here
            // says which scene, and says it as an error the caller can carry.
            return Err(anyhow!("a software scene has no buffer to render into"));
        }
        let mut fence: c_int = -1;
        // SAFETY: `self.scene` is non-null for the lifetime of `self`, and
        // `fence` is a live local for the length of the call.
        let result = unsafe { ffi::solium_qml_scene_render_gpu(self.scene, &raw mut fence) };
        match result {
            0 => Err(anyhow!("the GPU render failed")),
            UNCHANGED => Ok(None),
            _ if fence < 0 => Ok(Some(None)),
            // SAFETY: the host handed ownership of this fd to us, and sets it
            // to -1 — caught above — on every path where it did not.
            _ => Ok(Some(Some(unsafe { OwnedFd::from_raw_fd(fence) }))),
        }
    }

    /// Resize a scene to `width` by `height` **device** pixels, at `scale`
    /// device pixels to a logical one.
    ///
    /// The scene is laid out in logical units and rasterised at the full size,
    /// so a titlebar declared 32 pixels tall in QML is 32 *logical* pixels on
    /// every monitor and is drawn with as many real pixels as that monitor has.
    /// The alternative — laying out in device pixels — makes every hardcoded
    /// size in every QML file mean something different per monitor.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn resize(&mut self, width: i32, height: i32, scale: f64) {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        if width <= 0 || height <= 0 {
            return;
        }
        if self.size == (width, height) && (self.scale - scale).abs() < f64::EPSILON {
            return;
        }
        self.size = (width, height);
        self.scale = scale;
        // SAFETY: `self.scene` is non-null for the lifetime of `self`.
        unsafe { ffi::solium_qml_scene_resize(self.scene, width, height, scale) }
    }

    /// Advance QML animations to a point on the compositor's clock.
    ///
    /// Not Qt's clock: there is one clock here, and a QML animation running on
    /// a second one would drift against every transform beside it.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    /// Whether Qt has anything new to draw for this scene.
    ///
    /// Qt reports it through `renderRequested` and `sceneChanged`; asking is a
    /// flag read, so a screen of idle frames costs a comparison each.
    pub(crate) fn needs_render(&self) -> bool {
        // SAFETY: `self.scene` is non-null for the lifetime of `self`.
        unsafe { ffi::solium_qml_scene_dirty(self.scene) != 0 }
    }

    /// Render the scene if it has changed, then hand back its pixels.
    ///
    /// Premultiplied ARGB32 with the returned row stride, owned by the scene
    /// and valid until the next render or resize — hence the borrow. `changed`
    /// is false when Qt reported nothing to redraw, in which case the pixels
    /// are the previous frame's and do not need re-uploading.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn render(&mut self) -> Result<Rendered<'_>> {
        // SAFETY: `self.scene` is non-null for the lifetime of `self`.
        let status = unsafe { ffi::solium_qml_scene_render(self.scene) };
        if status == 0 {
            return Err(anyhow!("the QML scene failed to render"));
        }
        let changed = status != UNCHANGED;

        let mut stride: c_int = 0;
        // SAFETY: the scene rendered, so its image exists.
        let pixels = unsafe { ffi::solium_qml_scene_pixels(self.scene, &raw mut stride) };
        if pixels.is_null() || stride <= 0 {
            return Err(anyhow!("the QML scene produced no pixels"));
        }

        let stride = stride as usize;
        let height = usize::try_from(self.size.1.max(0)).unwrap_or_default();
        // SAFETY: the host guarantees `stride * height` readable bytes, owned
        // by the scene; the borrow ties them to `&mut self`, so the next render
        // cannot happen while they are held.
        let bytes = unsafe { std::slice::from_raw_parts(pixels, stride * height) };
        Ok(Rendered {
            changed,
            pixels: bytes,
            stride,
        })
    }

    /// Read a string property and clear it.
    ///
    /// The one direction state flows out of QML: a button writes it, the
    /// compositor takes it. A property both sides wrote would be two
    /// authorities over one piece of state, which is the mistake the old
    /// fork's drawer made.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn take_string(&mut self, name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        // SAFETY: `name` outlives the call.
        let value = unsafe { ffi::solium_qml_scene_take_string(self.scene, name.as_ptr()) };
        if value.is_null() {
            return None;
        }
        // SAFETY: non-null means the host stored a NUL-terminated string that
        // stays valid until the next call on this scene, and it is copied here.
        Some(
            unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn set_string(&mut self, name: &str, value: &str) {
        let (Ok(name), Ok(value)) = (CString::new(name), CString::new(value)) else {
            tracing::warn!(name, "property name or value contains a NUL byte");
            return;
        };
        // SAFETY: both strings outlive the call.
        unsafe { ffi::solium_qml_scene_set_string(self.scene, name.as_ptr(), value.as_ptr()) }
    }

    /// Set a whole-number property on the scene's root.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn set_int(&mut self, name: &str, value: i32) {
        let Ok(name) = std::ffi::CString::new(name) else {
            return;
        };
        // SAFETY: the scene is live for as long as `self`, and the name is a
        // NUL-terminated string that outlives the call.
        unsafe { ffi::solium_qml_scene_set_int(self.scene, name.as_ptr(), value) }
    }

    /// Read a whole-number property from the scene's root.
    #[expect(unsafe_code, reason = "as above")]
    pub(crate) fn get_int(&mut self, name: &str) -> i32 {
        let Ok(name) = std::ffi::CString::new(name) else {
            return 0;
        };
        // SAFETY: as above.
        unsafe { ffi::solium_qml_scene_get_int(self.scene, name.as_ptr()) }
    }

    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn set_bool(&mut self, name: &str, value: bool) {
        let Ok(name) = CString::new(name) else {
            tracing::warn!(name, "property name contains a NUL byte");
            return;
        };
        // SAFETY: `name` outlives the call.
        unsafe { ffi::solium_qml_scene_set_bool(self.scene, name.as_ptr(), c_int::from(value)) }
    }

    /// Read a bool property QML owns.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn get_bool(&self, name: &str) -> bool {
        let Ok(name) = CString::new(name) else {
            return false;
        };
        // SAFETY: `name` outlives the call.
        unsafe { ffi::solium_qml_scene_get_bool(self.scene, name.as_ptr()) != 0 }
    }

    /// Pointer input in scene coordinates. `None` is motion.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn pointer(&mut self, x: f64, y: f64, pressed: Option<bool>) {
        let pressed = match pressed {
            Some(true) => 1,
            Some(false) => 0,
            None => -1,
        };
        // SAFETY: `self.scene` is non-null for the lifetime of `self`.
        unsafe { ffi::solium_qml_scene_pointer(self.scene, x, y, pressed) }
    }
}

impl Drop for Scene {
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    fn drop(&mut self) {
        // The buffer goes after the scene, which is what the field order in the
        // struct buys: a `Drop` body runs before the fields are dropped. Qt's
        // teardown releases the EGLImage it made from the dmabuf, and doing
        // that while the fd it was made from is already closed is a question
        // not worth asking of a driver.
        //
        // On a GPU scene this leaves *no* GL context current — Qt makes its own
        // current to tear the RHI down and then releases it, whatever was
        // current on the way in. Measured, in both orderings. Anything holding
        // a context has to make it current again afterwards.
        //
        // SAFETY: freed exactly once, since `Scene` is not Clone and this
        // pointer is never handed out.
        unsafe { ffi::solium_qml_scene_free(self.scene) }
    }
}

impl std::fmt::Debug for Scene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Which of the two paths a scene is on is the first thing worth knowing
        // about it in a log, and it is not visible from the size.
        f.debug_struct("Scene")
            .field("size", &self.size)
            .field("gpu", &self.target.is_some())
            .finish()
    }
}
