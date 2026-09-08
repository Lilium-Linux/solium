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

mod target;

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
    let path = std::path::PathBuf::from(path);
    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| anyhow!("the QML import path contains a NUL byte"))?;

    // SAFETY: `path` outlives the call, and the shim is idempotent.
    if unsafe { ffi::solium_qml_start(path.as_ptr()) } == 0 {
        return Err(anyhow!("could not start Qt"));
    }
    Ok(())
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
        })
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
