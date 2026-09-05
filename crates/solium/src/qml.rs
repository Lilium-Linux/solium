//! Qt Quick scenes, hosted in this process.
//!
//! The compositor's shell surfaces — the bar now, window decorations next —
//! are authored in QML and rendered by Qt's scene graph inside this process.
//! See `qml/host.cpp` for why in-process (a shell painting frames over a
//! protocol was tried and measured at ~15 fps, 39% CPU) and why the *software*
//! rasteriser (no QPA plugin on this machine will adopt a foreign EGL context,
//! so a GL scene graph would render on a context Solium cannot sample from).
//!
//! This module is the only unsafe surface in the compositor, and it is kept
//! deliberately narrow: a handle, a render call, some setters. Everything Qt is
//! behind the C ABI.

use std::{
    ffi::{CStr, CString, c_char, c_double, c_int, c_longlong},
    path::Path,
    time::Duration,
};

use anyhow::{Result, anyhow};

#[expect(
    unsafe_code,
    reason = "the Qt host is C++; this is the declaration of its C ABI"
)]
mod ffi {
    use super::{c_char, c_double, c_int, c_longlong};

    #[repr(C)]
    pub(super) struct Scene {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        pub(super) fn solium_qml_start(import_path: *const c_char) -> c_int;
        pub(super) fn solium_qml_scene_new(
            qml_path: *const c_char,
            width: c_int,
            height: c_int,
            error: *mut *const c_char,
        ) -> *mut Scene;
        pub(super) fn solium_qml_scene_free(scene: *mut Scene);
        pub(super) fn solium_qml_scene_resize(scene: *mut Scene, width: c_int, height: c_int);
        pub(super) fn solium_qml_scene_advance(scene: *mut Scene, elapsed_ms: c_longlong);
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
        pub(super) fn solium_qml_scene_set_bool(
            scene: *mut Scene,
            name: *const c_char,
            value: c_int,
        );
        pub(super) fn solium_qml_scene_get_bool(scene: *const Scene, name: *const c_char) -> c_int;
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
    let path = std::path::PathBuf::from(path);
    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| anyhow!("the QML import path contains a NUL byte"))?;

    // SAFETY: `path` outlives the call, and the shim is idempotent.
    if unsafe { ffi::solium_qml_start(path.as_ptr()) } == 0 {
        return Err(anyhow!("could not start Qt"));
    }
    Ok(())
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
    std::ffi::OsString::from(format!("{own}:{shim}"))
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
    size: (i32, i32),
}

// The scene is bound to the GL context it was created on, and that context
// belongs to the render thread. Not Send, deliberately: sending it elsewhere
// would put Qt's scene graph on a thread with no current context.
impl Scene {
    /// Load a QML file into a scene of the given size.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn new(qml_path: &Path, width: i32, height: i32) -> Result<Self> {
        let path = CString::new(qml_path.as_os_str().as_encoded_bytes())
            .map_err(|_| anyhow!("the QML path contains a NUL byte"))?;
        let mut error: *const c_char = std::ptr::null();

        // SAFETY: `path` outlives the call; `error` is only read when the
        // call returns null, which is when the shim has set it.
        let scene =
            unsafe { ffi::solium_qml_scene_new(path.as_ptr(), width, height, &raw mut error) };

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
        })
    }

    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn resize(&mut self, width: i32, height: i32) {
        if self.size == (width, height) || width <= 0 || height <= 0 {
            return;
        }
        self.size = (width, height);
        // SAFETY: `self.scene` is non-null for the lifetime of `self`.
        unsafe { ffi::solium_qml_scene_resize(self.scene, width, height) }
    }

    /// Advance QML animations to a point on the compositor's clock.
    ///
    /// Not Qt's clock: there is one clock here, and a QML animation running on
    /// a second one would drift against every transform beside it.
    #[expect(unsafe_code, reason = "calling into the Qt host")]
    pub(crate) fn advance(&mut self, elapsed: Duration) {
        let millis = c_longlong::try_from(elapsed.as_millis()).unwrap_or(c_longlong::MAX);
        // SAFETY: `self.scene` is non-null for the lifetime of `self`.
        unsafe { ffi::solium_qml_scene_advance(self.scene, millis) }
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
        // SAFETY: freed exactly once, since `Scene` is not Clone and this
        // pointer is never handed out.
        unsafe { ffi::solium_qml_scene_free(self.scene) }
    }
}

impl std::fmt::Debug for Scene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scene").field("size", &self.size).finish()
    }
}
