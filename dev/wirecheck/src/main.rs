//! The *compositor's* side of the join, on real hardware.
//!
//! joincheck proved Qt can fill a buffer we allocated. This proves the
//! compositor can then put it on screen, by running the exact sequence
//! `ShellSurface::on_gpu` runs, against a real smithay `GlesRenderer` built the
//! way `tty::State::open_gpu` builds one:
//!
//!   scene_new_gpu -> restore our EGL context -> render_gpu -> restore again
//!   -> EGLFence::import -> Renderer::wait -> import_dmabuf
//!   -> TextureRenderElement::draw into an offscreen target -> read back
//!
//! The picture is checked against the *memory* path rather than against an
//! absolute orientation: the same known image is uploaded with `import_memory`
//! (which is top-down by definition, and is what the software shell path uses)
//! and drawn through the identical element parameters. Two readbacks that match
//! byte for byte mean the GPU buffer is stored the same way the memory one is,
//! whatever convention the offscreen target itself has -- it cancels.
//!
//! Rust side: the crate's own `crates/solium/src/qml/target.rs`, by path.
//! C++ side: the crate's own `crates/solium/qml/host.cpp`, compiled from source.

use anyhow::{Context, Result, anyhow};
use smithay::backend::allocator::{Buffer as _, Fourcc};
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::egl::fence::EGLFence;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element as _, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{
    Bind as _, Color32F, ExportMem as _, Frame as _, ImportDma as _, ImportMem as _, Offscreen as _,
    Renderer as _,
};
use smithay::utils::{DeviceFd, Rectangle, Scale, Transform};
use std::ffi::{CString, c_char, c_int, c_uint, c_void};
use std::os::fd::{FromRawFd as _, OwnedFd};

// The crate's own allocator, included by path relative to this file rather
// than copied: `allocate` and `as_ffi` here are the real functions.
#[path = "../../../crates/solium/src/qml/target.rs"]
#[allow(dead_code, reason = "target.rs carries fields only the compositor reads")]
mod target;

unsafe extern "C" {
    fn solium_qml_start_gpu(import_path: *const c_char) -> c_int;
    fn solium_qml_scene_new_gpu(
        qml_path: *const c_char,
        width: c_int,
        height: c_int,
        dmabuf_fd: c_int,
        stride: c_int,
        modifier: u64,
        fourcc: c_uint,
        initial_json: *const c_char,
    ) -> *mut c_void;
    fn solium_qml_scene_render_gpu(scene: *mut c_void, fence_fd: *mut c_int) -> c_int;
    fn solium_qml_scene_resize(scene: *mut c_void, width: c_int, height: c_int, scale: f64);
    fn solium_qml_scene_rebind(
        scene: *mut c_void,
        dmabuf_fd: c_int,
        stride: c_int,
        modifier: u64,
        fourcc: c_uint,
        width: c_int,
        height: c_int,
        scale: f64,
    ) -> bool;
    fn solium_qml_scene_set_int(scene: *mut c_void, name: *const c_char, value: c_int);
    fn solium_qml_scene_set_bool(scene: *mut c_void, name: *const c_char, value: c_int);
    fn solium_qml_scene_get_int(scene: *mut c_void, name: *const c_char) -> c_int;
    fn solium_qml_scene_free(scene: *mut c_void);
    fn solium_qml_scene_animating(scene: *const c_void) -> c_int;
    fn solium_qml_tick(elapsed_ms: i64);

    fn wirecheck_belief_names_scene(scene: *mut c_void) -> c_int;
    fn wirecheck_egl_agrees_with(scene: *mut c_void) -> c_int;
    fn wirecheck_anything_animating() -> c_int;

    fn join_readback(
        node: *const c_char,
        dmabuf_fd: c_int,
        w: c_int,
        h: c_int,
        stride: c_int,
        modifier: u64,
        fourcc: c_uint,
        out: *mut u8,
    ) -> c_int;
}

/// Qt's diagnostics, for a harness that has no `tracing` in it.
///
/// `host.cpp` installs a Qt message handler that calls this, and the compositor
/// defines it in `crates/solium/src/qml.rs` with one `tracing` macro per level.
/// Nothing in this binary links that crate, so the symbol has to exist here
/// too — and it has to *print*, because the render control's entire content is
/// Qt saying it could not build a render target, and a control whose
/// diagnostics vanish is a control nobody can read.
///
/// The `qt` prefix and the bracketed category are the only difference from what
/// Qt's own handler used to put on these lines. Nothing in README.md's
/// expectations reads them: every line quoted there is one this harness prints
/// about its own assertions.
#[unsafe(no_mangle)]
extern "C" fn solium_qml_log_from_qt(
    level: c_int,
    category: *const c_char,
    message: *const c_char,
    _file: *const c_char,
    _line: c_int,
    _function: *const c_char,
) {
    // Borrowed for the call and no longer, exactly as host.h says.
    let borrowed = |ptr: *const c_char| -> String {
        if ptr.is_null() {
            return String::new();
        }
        // SAFETY: host.cpp passes a QMessageLogContext's own pointers, or the
        // bytes of a QByteArray that outlives the call.
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    };
    // The SOLIUM_QML_LOG_* values from crates/solium/qml/host.h.
    let name = match level {
        0 => "debug",
        1 => "info",
        3 => "error",
        _ => "warn",
    };
    eprintln!("qt {name} [{}]: {}", borrowed(category), borrowed(message));
}

/// This checkout, from where the binary was compiled rather than from where it
/// happens to be run.
fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf()
}

/// Everything Qt must be told before it exists, so it touches no hardware.
///
/// The same four settings `qml::keep_qt_off_the_hardware` writes, for the same
/// reason and with the same measurements behind them: eglfs will otherwise open
/// the card node and take DRM master from whatever session is running. Done
/// here rather than left to the caller so this is one binary to run and not a
/// binary plus a remembered environment.
fn keep_qt_off_the_hardware(node: &str) -> Result<()> {
    let config = std::env::temp_dir().join("wirecheck-eglfs-kms.json");
    std::fs::write(
        &config,
        format!("{{ \"device\": \"{node}\", \"headless\": \"64x64\" }}\n"),
    )
    .with_context(|| format!("writing {}", config.display()))?;
    // SAFETY: single-threaded, before Qt exists, and nothing else here reads
    // the environment.
    unsafe {
        std::env::set_var("QT_QPA_PLATFORM", "eglfs");
        std::env::set_var("QT_QPA_EGLFS_KMS_CONFIG", &config);
        std::env::set_var("QT_QPA_EGLFS_DISABLE_INPUT", "1");
        std::env::set_var("QT_QPA_EGLFS_KMS_NO_EVENT_READER_THREAD", "1");
        std::env::set_var("QT_QPA_NO_SIGNAL_HANDLER", "1");
        std::env::set_var("QT_QPA_ENABLE_TERMINAL_KEYBOARD", "1");
        // Qt's warnings went to journald on Fedora otherwise, which was the
        // whole diagnostic half of host.cpp invisible. See dev/README.md.
        //
        // Redundant now that host.cpp installs its own message handler and
        // `solium_qml_log_from_qt` above prints them: Qt's default handler is
        // the only thing this variable steers, and nothing reaches it any more.
        // Kept because it covers the window before the handler is installed,
        // and because a harness that stops printing Qt's own words is a thing
        // this project should have to decide on rather than inherit.
        std::env::set_var("QT_FORCE_STDERR_LOGGING", "1");
    }
    Ok(())
}

/// What the probe QML paints, top-down, as ARGB8888 little-endian (B,G,R,A).
///
/// The same four quadrants joincheck uses: top-left blue, top-right green,
/// bottom-left white, bottom-right red.
fn expected_argb(w: i32, h: i32) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let px: [u8; 4] = match (y >= h / 2, x >= w / 2) {
                (false, false) => [255, 0, 0, 255],     // blue
                (false, true) => [0, 255, 0, 255],      // green
                (true, false) => [255, 255, 255, 255],  // white
                (true, true) => [0, 0, 255, 255],       // red
            };
            let i = ((y * w + x) * 4) as usize;
            out[i..i + 4].copy_from_slice(&px);
        }
    }
    out
}


/// A distinct picture per guard texture, so a name that got swapped for another
/// is as visible as a name that got deleted.
fn guard_image(index: usize, w: i32, h: i32) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 4) as usize];
    #[allow(clippy::cast_possible_truncation)]
    let seed = (index as u8).wrapping_mul(37).wrapping_add(29);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            // Never all-zero: a deleted texture samples as zeros, and a guard
            // that was legitimately zero could not tell the two apart.
            out[i] = seed | 0x11;
            #[allow(clippy::cast_possible_truncation)]
            {
                out[i + 1] = (x as u8) | 0x21;
                out[i + 2] = (y as u8) | 0x41;
            }
            out[i + 3] = 255;
        }
    }
    out
}


/// Which GL object names exist in the compositor's context right now.
///
/// The direct instrument. "Did the picture change" only catches a delete that
/// happens to hit a name we are still sampling; this catches every delete, of
/// every object kind, whether or not anything was using it.
fn gl_names(renderer: &mut GlesRenderer, upto: u32) -> Result<Vec<(char, u32)>> {
    renderer
        .with_context(|gl| {
            let mut live = Vec::new();
            for name in 1..=upto {
                unsafe {
                    if gl.IsTexture(name) != 0 {
                        live.push(('t', name));
                    }
                    if gl.IsBuffer(name) != 0 {
                        live.push(('b', name));
                    }
                    if gl.IsFramebuffer(name) != 0 {
                        live.push(('f', name));
                    }
                    if gl.IsProgram(name) != 0 {
                        live.push(('p', name));
                    }
                    if gl.IsRenderbuffer(name) != 0 {
                        live.push(('r', name));
                    }
                }
            }
            live
        })
        .map_err(|err| anyhow!("gl_names: {err}"))
}

/// Empty the GL error queue, and say what was in it.
///
/// `glGetError` pops **one** error per call and clears it, so a probe that does
/// not drain first reports whatever the last few hundred lines happened to
/// leave behind and attributes it to the operation it is sitting under. That is
/// not hypothetical: under the render control the queue already holds 0x501
/// before the rebind, and the probe after the rebind reported it as the
/// rebind's -- failing the run with a message naming code that is fine and
/// sending the reader there.
///
/// So each probed operation is bracketed: drain to zero immediately before it,
/// read once immediately after it. Anything the drain finds is printed rather
/// than fatal -- it belongs to something further up, and failing here would be
/// the same misattribution in the other direction.
fn drain_gl_errors(renderer: &mut GlesRenderer, before: &str) -> Result<()> {
    let found = renderer
        .with_context(|gl| {
            let mut found = Vec::new();
            // Bounded: a context that never returns GL_NO_ERROR would otherwise
            // spin here for ever, which is a worse failure than a missed error.
            for _ in 0..32 {
                let error = unsafe { gl.GetError() };
                if error == 0 {
                    break;
                }
                found.push(format!("0x{error:x}"));
            }
            found
        })
        .map_err(|err| anyhow!("draining GL errors: {err}"))?;
    if !found.is_empty() {
        println!("  !! GL error(s) {found:?} already pending BEFORE {before}; drained, not theirs");
    }
    Ok(())
}

fn open_gbm(path: &str) -> Result<GbmDevice<DrmDeviceFd>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {path}"))?;
    GbmDevice::new(DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file))))
        .context("creating the GBM device")
}

/// `surface.rs::restore`, verbatim in behaviour.
fn restore(renderer: &GlesRenderer) -> Result<()> {
    unsafe { renderer.egl_context().make_current() }
        .map_err(|err| anyhow!("making our EGL context current again: {err}"))
}

/// `surface.rs::wait_for`, verbatim in behaviour.
fn wait_for(renderer: &mut GlesRenderer, fence: OwnedFd) -> Result<()> {
    let imported = {
        let display = renderer.egl_context().display();
        EGLFence::import(display, fence).map_err(|err| anyhow!("importing Qt's fence: {err}"))?
    };
    renderer
        .wait(&SyncPoint::from(imported))
        .map_err(|err| anyhow!("waiting on Qt's fence: {err}"))
}

/// Clear the scene's buffer to transparent, through the compositor's renderer.
///
/// `glFinish` rather than a fence: this has to have landed before Qt is asked
/// to draw over it, and the point of the wipe is defeated by racing it.
fn wipe(renderer: &mut GlesRenderer, buffer: &smithay::backend::allocator::dmabuf::Dmabuf, w: i32, h: i32) -> Result<()> {
    let mut buffer = buffer.clone();
    {
        let mut framebuffer = renderer
            .bind(&mut buffer)
            .map_err(|err| anyhow!("binding the scene buffer to wipe it: {err}"))?;
        let mut frame = renderer
            .render(&mut framebuffer, (w, h).into(), Transform::Normal)
            .map_err(|err| anyhow!("wiping: {err}"))?;
        frame
            .clear(Color32F::TRANSPARENT, &[Rectangle::from_size((w, h).into())])
            .map_err(|err| anyhow!("clearing: {err}"))?;
        let _ = frame.finish().map_err(|err| anyhow!("finishing the wipe: {err}"))?;
    }
    renderer
        .with_context(|gl| unsafe { gl.Finish() })
        .map_err(|err| anyhow!("finishing the wipe: {err}"))?;
    Ok(())
}

/// Draw one element into a fresh offscreen texture and read the pixels back.
///
/// The same shape as `offscreen::capture`: bind, render, draw, copy back.
fn draw_and_read(
    renderer: &mut GlesRenderer,
    element: &TextureRenderElement<GlesTexture>,
    w: i32,
    h: i32,
    scale: f64,
) -> Result<Vec<u8>> {
    let mut into: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (w, h).into())
        .map_err(|err| anyhow!("offscreen buffer: {err}"))?;
    let size = (w, h).into();
    {
        let mut framebuffer = renderer
            .bind(&mut into)
            .map_err(|err| anyhow!("binding the offscreen buffer: {err}"))?;
        let mut frame = renderer
            .render(&mut framebuffer, size, Transform::Normal)
            .map_err(|err| anyhow!("starting the frame: {err}"))?;
        frame
            .clear(Color32F::TRANSPARENT, &[Rectangle::from_size(size)])
            .map_err(|err| anyhow!("clearing: {err}"))?;
        let whole = [Rectangle::from_size(size)];
        let src = element.src();
        let dst = element.geometry(Scale::from(scale));
        <TextureRenderElement<GlesTexture> as RenderElement<GlesRenderer>>::draw(
            element, &mut frame, src, dst, &whole, &[],
        )
        .map_err(|err| anyhow!("drawing the element: {err}"))?;
        let _ = frame.finish().map_err(|err| anyhow!("finishing: {err}"))?;
    }
    let framebuffer = renderer
        .bind(&mut into)
        .map_err(|err| anyhow!("re-binding to read back: {err}"))?;
    let mapping = renderer
        .copy_framebuffer(&framebuffer, Rectangle::from_size((w, h).into()), Fourcc::Argb8888)
        .map_err(|err| anyhow!("copying the framebuffer: {err}"))?;
    drop(framebuffer);
    let pixels = renderer
        .map_texture(&mapping)
        .map_err(|err| anyhow!("mapping: {err}"))?
        .to_vec();
    Ok(pixels)
}

/// Read a dmabuf straight back, with no element and no draw in between.
///
/// Import it as a texture, bind that as a framebuffer, copy it out. What it
/// answers is "is there anything in this buffer", which is the whole question
/// for a scene whose picture this harness has no reference for.
fn read_dmabuf(
    renderer: &mut GlesRenderer,
    buffer: &smithay::backend::allocator::dmabuf::Dmabuf,
    w: i32,
    h: i32,
) -> Result<Vec<u8>> {
    let mut texture = renderer
        .import_dmabuf(buffer, None)
        .map_err(|err| anyhow!("import_dmabuf for a readback: {err}"))?;
    let framebuffer = renderer
        .bind(&mut texture)
        .map_err(|err| anyhow!("binding a buffer to read it back: {err}"))?;
    let mapping = renderer
        .copy_framebuffer(&framebuffer, Rectangle::from_size((w, h).into()), Fourcc::Argb8888)
        .map_err(|err| anyhow!("copying a buffer to read it back: {err}"))?;
    drop(framebuffer);
    Ok(renderer
        .map_texture(&mapping)
        .map_err(|err| anyhow!("mapping: {err}"))?
        .to_vec())
}

fn element_for(
    renderer: &GlesRenderer,
    texture: GlesTexture,
    pixels: (i32, i32),
    logical: (i32, i32),
    scale: f64,
) -> TextureRenderElement<GlesTexture> {
    // Exactly `ShellSurface::on_gpu`'s parameters, at area.loc = (0, 0).
    let source = Rectangle::from_size((f64::from(pixels.0), f64::from(pixels.1)).into());
    TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        (0.0 * scale, 0.0 * scale),
        texture,
        1,
        Transform::Normal,
        Some(1.0),
        Some(source),
        Some(logical.into()),
        None,
        Kind::Unspecified,
    )
}

/// The counter `quadrants.qml` carries, as the object tree currently holds it.
fn frames_of(scene: *mut c_void) -> i32 {
    unsafe { solium_qml_scene_get_int(scene, c"frames".as_ptr()) }
}

/// A compositor frame's worth of time, near enough. The exact figure does not
/// matter; that the clock moves by a fixed amount each time does.
const FRAME_MS: i64 = 16;

/// Advance the compositor's clock, which is the only thing that moves a QML
/// animation in this process.
///
/// `render::prepare` calls this once per frame for the whole process. Nothing
/// in this harness did until the animation assertion in the resize case needed
/// one: an animation that is never advanced never changes, so "did it move
/// across the rebind" asked of an unticked scene is a question about a clock
/// that never ran, and would have answered "no" for the wrong reason.
fn tick(clock: &mut i64, by: i64) {
    *clock += by;
    unsafe { solium_qml_tick(*clock) };
}

/// Where `quadrants.qml`'s running animation has got to.
///
/// Read out of the object tree, like `frames`, and for a stronger reason: a
/// rebuilt tree does not merely forget a number, it starts a *new* animation
/// from its declared `from:`. So a tree that was replaced reads back near zero
/// however long the clock has been running.
fn spin_of(scene: *mut c_void) -> i32 {
    unsafe { solium_qml_scene_get_int(scene, c"spin".as_ptr()) }
}

/// Whether the host says an animation is running inside this scene.
///
/// The compositor's whole animation loop now hangs on this one answer -- a
/// decoration is drawn again only while `dirty || animating` -- and it is a
/// claim about *Qt*, not about anything in this repository: that a
/// `QQuickAbstractAnimation` driven by a `Behavior` or by `NumberAnimation on`
/// sets its own `running` property, and clears it when it is done. Nothing in
/// `cargo test` can ask, because asking needs a live Qt.
///
/// Checked both ways in this run and that is the whole of its negative control,
/// so it needs no edited copy of anything: `quadrants.qml` holds an animation
/// with `loops: Animation.Infinite` and must read 1 for the length of the run,
/// while `cursor.qml` and `decorations/top.qml` are built and rendered with
/// nothing written to them and must read 0. A stub answering "yes" fails on the
/// second, one answering "no" fails on the first, and an answer read off the
/// process rather than the scene -- `QAnimationDriver::isRunning()`, which is
/// the obvious wrong answer -- fails on the second too, because by then
/// `quadrants.qml` has been animating for the whole run.
fn animating(scene: *mut c_void) -> bool {
    unsafe { solium_qml_scene_animating(scene.cast_const()) != 0 }
}

/// Does an animation started from a settled desktop actually *run*, or does it
/// land on its final value in one step?
///
/// The question the compositor's animation clock decides, and the one thing
/// `solium_qml_scene_animating` cannot answer: a scene can say "yes, animating"
/// on every tick of an animation that is being advanced 650ms at a time and has
/// therefore already finished. On the hardware that is a titlebar that
/// sometimes slides out and sometimes is simply *there*, at its final position,
/// with no animation at all.
///
/// Qt's contract for `QAnimationDriver::elapsed()` is "the number of
/// milliseconds since the animations was started" -- its own driver returns
/// `d->running ? d->timer.elapsed() : 0`, restarted inside `start()`. It relies
/// on that: when the process goes from no animations to one,
/// `QUnifiedTimer::startTimers` zeroes `lastTick` and `driverStartTime` (qtbase
/// v6.11.2, `src/corelib/animation/qabstractanimation.cpp:378-389`), so the
/// first delta a brand new animation is given is whatever `elapsed()` reads
/// right then. A driver reporting the compositor's uptime hands it the whole
/// uptime, and any animation shorter than the session is over before its first
/// drawn frame.
///
/// Which is why this has to run **first**, before any other scene exists. The
/// zeroing happens only on the empty-to-non-empty edge, and `quadrants.qml`'s
/// `Animation.Infinite` holds the registry open from the moment it is built
/// until the process exits -- so on any later line the reset never happens, the
/// deltas are all 16ms, and this case would pass with the defect present. The
/// precondition is asserted through `wirecheck_anything_animating` rather than
/// left to this comment.
///
/// The assertion is that the value passes *through the middle*. Not that it
/// reaches its end -- it does that either way, instantly, which is the bug --
/// and not an exact trajectory, which would be a test of Qt's easing curve.
/// `appear.qml` slides 34 units over 260ms on a linear curve, so with a 16ms
/// frame roughly sixteen readings must land strictly between the two ends; one
/// is enough to prove it interpolated.
///
/// The scene is kept alive and handed back rather than freed. Freeing it here
/// would reach C-1's defect in C-1's own ordering, before C-1's census is
/// taken, which is the same reason the scene case below keeps its two.
fn appear_animation(
    gbm: &GbmDevice<DrmDeviceFd>,
    renderer: &GlesRenderer,
    clock: &mut i64,
) -> Result<(*mut c_void, target::Target)> {
    println!("\n=== an appear animation, on a desktop where nothing else is moving ===");
    const SIZE: i32 = 64;
    let buffer = target::allocate(gbm, SIZE, SIZE).context("the appear scene's buffer")?;
    let (fd, stride, modifier, fourcc) = buffer.as_ffi().context("as_ffi")?;
    let path = CString::new(
        repo()
            .join("dev/wirecheck/appear.qml")
            .as_os_str()
            .as_encoded_bytes(),
    )?;
    let scene = unsafe {
        solium_qml_scene_new_gpu(
            path.as_ptr(),
            SIZE,
            SIZE,
            fd,
            stride,
            modifier,
            fourcc,
            std::ptr::null(),
        )
    };
    restore(renderer)?;
    if scene.is_null() {
        return Err(anyhow!("a GPU host could not build dev/wirecheck/appear.qml"));
    }

    // A window that has been sitting there. Forty frames of the compositor
    // drawing and ticking, which is also what puts the clock past this
    // animation's own duration -- the defect is invisible below it, and a
    // harness whose clock starts at zero is a harness that cannot see it. Real
    // uptime when somebody hovers a window is minutes, not milliseconds.
    let travel = unsafe { solium_qml_scene_get_int(scene, c"travel".as_ptr()) };
    for _ in 0..40 {
        tick(clock, FRAME_MS);
    }
    let settled = unsafe { solium_qml_scene_get_int(scene, c"slid".as_ptr()) };
    println!(
        "  settled after 40 frames: slid = {settled} of -{travel}..0, clock = {clock}ms, \
         scene animating = {}",
        animating(scene)
    );
    if settled != -travel {
        return Err(anyhow!(
            "the appear scene did not settle at its starting value: slid reads {settled}, \
             want -{travel}. This case measures an animation from rest and there is no rest"
        ));
    }
    // Both preconditions. The scene's own, and the process's -- which is the one
    // that decides whether this case is testing anything at all.
    if animating(scene) || unsafe { wirecheck_anything_animating() } != 0 {
        return Err(anyhow!(
            "something in this process is already animating before the appear case writes \
             anything. Qt zeroes its animation reference only on the edge from no animations \
             to one, so with the registry already open this case cannot fail however broken \
             the clock is. A scene built ahead of this one is the way that happens"
        ));
    }

    // `Decoration::tell`: the property write that starts the `Behavior`.
    unsafe { solium_qml_scene_set_bool(scene, c"pointerInside".as_ptr(), 1) };
    // Read where `Drawn::drawing` reads it, before the draw -- and on the write
    // frame, which is the frame the shipped code already gets right. Recorded
    // rather than asserted on its own: it was `true` here both before and after
    // the fix, so it separates nothing, and printing it is what stops the next
    // reader assuming this case is about that.
    println!(
        "  the frame that writes pointerInside: scene animating = {}, slid = {}",
        animating(scene),
        unsafe { solium_qml_scene_get_int(scene, c"slid".as_ptr()) }
    );

    // Twenty frames is 320ms, comfortably past the 260ms the animation lasts.
    let mut readings = Vec::new();
    for _ in 0..20 {
        tick(clock, FRAME_MS);
        readings.push(unsafe { solium_qml_scene_get_int(scene, c"slid".as_ptr()) });
    }
    println!("  slid, frame by frame: {readings:?}");
    let midway = readings
        .iter()
        .filter(|slid| **slid > -travel && **slid < 0)
        .count();
    println!("  readings strictly between -{travel} and 0: {midway}");
    if midway == 0 {
        return Err(anyhow!(
            "the appear animation never took a step: `slid` went {:?}, from -{travel} to 0 \
             with nothing in between, over {} frames of a {}ms animation. The animation was \
             advanced past its own end in a single tick, so the bar does not slide out -- it \
             is simply there. `solium_qml_scene_animating` cannot see this: it says `true` \
             throughout, which is why the compositor draws exactly the frames it should and \
             every one of them shows the finished value",
            readings,
            readings.len(),
            260,
        ));
    }
    if readings.last() != Some(&0) {
        return Err(anyhow!(
            "the appear animation did not finish: `slid` went {readings:?} and ends at {:?} \
             rather than 0, so the clock is now advancing too slowly rather than too fast",
            readings.last()
        ));
    }
    Ok((scene, buffer))
}

/// Advance that counter by one, and hand back what it now reads.
///
/// Deliberately round-trips through the QML item rather than counting here: the
/// count is *stored in the object tree and nowhere else*, so a tree that was
/// rebuilt hands back the property's declared default and the count restarts at
/// zero. Counting on this side would survive a rebuild and measure nothing,
/// which is the failure mode this whole harness keeps finding in itself.
fn bump(scene: *mut c_void) -> i32 {
    let next = frames_of(scene) + 1;
    unsafe { solium_qml_scene_set_int(scene, c"frames".as_ptr(), next) };
    next
}

/// Make a scene dirty without changing what it lays out to.
///
/// A scale change and back leaves the same geometry and the same picture, so
/// what follows is a render of an unchanged scene rather than of a different
/// one. `solium_qml_scene_render_gpu` returns SOLIUM_QML_UNCHANGED otherwise
/// and never touches the buffer.
fn poke(scene: *mut c_void, pixels: i32, scale: f64) {
    unsafe { solium_qml_scene_resize(scene, pixels, pixels, scale * 2.0) };
    unsafe { solium_qml_scene_resize(scene, pixels, pixels, scale) };
}

fn differing(a: &[u8], b: &[u8]) -> (usize, Option<usize>) {
    let mut bad = 0;
    let mut first = None;
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            bad += 1;
            if first.is_none() {
                first = Some(i);
            }
        }
    }
    (bad, first)
}

fn main() -> Result<()> {
    let node = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/dri/renderD128".to_owned());
    let scale: f64 = std::env::var("WIRECHECK_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let logical: i32 = std::env::var("WIRECHECK_LOGICAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let skip_restore = std::env::var_os("WIRECHECK_NO_RESTORE").is_some();
    // The compositor's clock, advanced by hand. See `tick`.
    let mut clock: i64 = 0;
    #[allow(clippy::cast_possible_truncation)]
    let pixels = ((f64::from(logical) * scale).round() as i32).max(1);
    println!(
        "=== the compositor's renderer, {logical} logical -> {pixels} device px at scale {scale} ==="
    );
    if skip_restore {
        println!("  !! WIRECHECK_NO_RESTORE: the context restore is deliberately skipped");
    }

    keep_qt_off_the_hardware(&node)?;

    // The buffers' device.
    let gbm = open_gbm(&node)?;

    // The compositor's renderer, built as tty::State::open_gpu builds it. In
    // the real compositor it comes up *after* Qt (scripts load, a scene starts
    // Qt, and only then does open_gpu run) and on a device of its own -- the
    // card node, opened through the session. Both are knobs here so the
    // difference can be bisected rather than argued about.
    let late = std::env::var_os("WIRECHECK_LATE_RENDERER").is_some();
    let separate = std::env::var_os("WIRECHECK_SEPARATE_GBM").is_some();
    let renderer_node = std::env::var("WIRECHECK_RENDERER_NODE").unwrap_or_else(|_| node.clone());
    let make_renderer = |gbm: &GbmDevice<DrmDeviceFd>| -> Result<GlesRenderer> {
        let own;
        let device = if separate {
            own = open_gbm(&renderer_node)?;
            &own
        } else {
            gbm
        };
        let display = unsafe { EGLDisplay::new(device.clone()) }.context("EGL display")?;
        let context = EGLContext::new(&display).context("EGL context")?;
        Ok(unsafe { GlesRenderer::new(context) }.context("GlesRenderer")?)
    };
    println!(
        "  renderer: {} Qt, on {} device ({renderer_node})",
        if late { "after" } else { "before" },
        if separate { "its own" } else { "the buffers'" }
    );
    let mut renderer = if late {
        // Qt first, exactly as the compositor orders it.
        let import_path = CString::new(repo().join("crates/solium/qml").as_os_str().as_encoded_bytes())?;
        if unsafe { solium_qml_start_gpu(import_path.as_ptr()) } != 1 {
            return Err(anyhow!("start_gpu refused"));
        }
        make_renderer(&gbm)?
    } else {
        make_renderer(&gbm)?
    };
    println!("  our GlesRenderer is up");
    // Taken before Qt exists, so anything in it is unambiguously the
    // compositor's. Without this the census at the end could be waved away as
    // Qt having made those objects itself.
    let live_before_qt = if late {
        Vec::new() // Qt is already up in this mode; the census would prove nothing.
    } else {
        gl_names(&mut renderer, 64)?
    };
    println!("  GL objects that exist before Qt is started at all: {live_before_qt:?}");

    // The reference: the same picture through the path the software shell uses.
    let reference_texture = renderer
        .import_memory(
            &expected_argb(pixels, pixels),
            Fourcc::Argb8888,
            (pixels, pixels).into(),
            false,
        )
        .map_err(|err| anyhow!("import_memory: {err}"))?;
    let reference_element = element_for(
        &renderer,
        reference_texture,
        (pixels, pixels),
        (logical, logical),
        scale,
    );
    let reference = draw_and_read(&mut renderer, &reference_element, pixels, pixels, scale)?;
    println!("  reference drawn through import_memory ({} bytes)", reference.len());

    // Qt, on the GPU (already up when WIRECHECK_LATE_RENDERER is set).
    if !late {
        let import_path = CString::new(repo().join("crates/solium/qml").as_os_str().as_encoded_bytes())?;
        if unsafe { solium_qml_start_gpu(import_path.as_ptr()) } != 1 {
            return Err(anyhow!("start_gpu refused"));
        }
    }
    // ------------------------------------------------------------------
    // An appear animation, started on a desktop where nothing else is moving.
    //
    // First in the run, and it has to be first: the defect it looks for exists
    // only while the process has *no* animation registered at all, and the very
    // next scene built below leaves one running for the rest of the run. See
    // `appear_animation`, which asserts that precondition rather than trusting
    // this comment to stay true.
    // Both bindings are held to the end of the run and neither is read again:
    // the scene is deliberately not freed (see `appear_animation`), and the
    // buffer under it closes its dmabuf fd when it drops, so it has to outlive
    // the scene that is still pointed at it.
    let (_appearing, _appear_buffer) = appear_animation(&gbm, &renderer, &mut clock)?;

    let scene_target = target::allocate(&gbm, pixels, pixels).context("target::allocate")?;
    let (fd, stride, modifier, fourcc) = scene_target.as_ffi().context("as_ffi")?;
    let qml = match std::env::var_os("WIRECHECK_QML") {
        Some(path) => std::path::PathBuf::from(path),
        None => repo().join("dev/wirecheck/quadrants.qml"),
    };
    let qml = CString::new(qml.as_os_str().as_encoded_bytes())?;
    let scene = unsafe {
        solium_qml_scene_new_gpu(
            qml.as_ptr(),
            pixels,
            pixels,
            fd,
            stride,
            modifier,
            fourcc,
            std::ptr::null(),
        )
    };
    if scene.is_null() {
        return Err(anyhow!("scene_new_gpu returned NULL"));
    }
    // Qt's `initialize()` has made Qt's context current on this thread.
    let early = !skip_restore && std::env::var("WIRECHECK_RESTORE_EARLY").as_deref() != Ok("0");
    if early {
        restore(&renderer)?;
    }
    println!(
        "  scene built; our context {} after Qt's initialize()",
        if early { "restored" } else { "LEFT WITH QT'S" }
    );

    // Scale only: the pixel size is the buffer's.
    unsafe { solium_qml_scene_resize(scene, pixels, pixels, scale) };

    let mut fence_fd: c_int = -1;
    let rendered = unsafe { solium_qml_scene_render_gpu(scene, &raw mut fence_fd) };
    if rendered != 1 {
        return Err(anyhow!("render_gpu returned {rendered}"));
    }
    if !skip_restore {
        restore(&renderer)?;
    }
    println!("  Qt rendered; fence fd = {fence_fd}; our context restored");

    if fence_fd >= 0 {
        let fence = unsafe { OwnedFd::from_raw_fd(fence_fd) };
        wait_for(&mut renderer, fence)?;
        println!("  EGLFence::import + Renderer::wait: OK");
    } else {
        println!("  no fence: the host waited with glFinish");
    }

    // Control, before Qt's buffer is touched: allocate a second buffer of the
    // same size and modifier, fill it *with smithay* by binding the dmabuf and
    // clearing it, then import that same dmabuf back as a texture and read it.
    // If this comes back empty too, smithay cannot import this modifier at all
    // and Qt has nothing to do with it.
    {
        let mine = target::allocate(&gbm, pixels, pixels).context("control buffer")?;
        let mut buffer = mine.dmabuf.clone();
        {
            let mut framebuffer = renderer
                .bind(&mut buffer)
                .map_err(|err| anyhow!("binding our own dmabuf: {err}"))?;
            let mut frame = renderer
                .render(&mut framebuffer, (pixels, pixels).into(), Transform::Normal)
                .map_err(|err| anyhow!("rendering into our own dmabuf: {err}"))?;
            frame
                .clear(Color32F::new(0.0, 1.0, 0.0, 1.0), &[Rectangle::from_size((pixels, pixels).into())])
                .map_err(|err| anyhow!("clearing: {err}"))?;
            let _ = frame.finish().map_err(|err| anyhow!("finishing: {err}"))?;
        }
        let control = renderer
            .import_dmabuf(&mine.dmabuf, None)
            .map_err(|err| anyhow!("import_dmabuf on our own buffer: {err}"))?;
        let mut control = control;
        let framebuffer = renderer
            .bind(&mut control)
            .map_err(|err| anyhow!("binding the control texture: {err}"))?;
        let mapping = renderer
            .copy_framebuffer(&framebuffer, Rectangle::from_size((pixels, pixels).into()), Fourcc::Argb8888)
            .map_err(|err| anyhow!("copying the control texture: {err}"))?;
        drop(framebuffer);
        let raw = renderer.map_texture(&mapping).map_err(|err| anyhow!("mapping: {err}"))?.to_vec();
        println!(
            "  CONTROL smithay writes a dmabuf and re-imports it: {} of {} bytes non-zero, first px {:?}",
            raw.iter().filter(|b| **b != 0).count(), raw.len(), &raw[..4]
        );
    }

    drain_gl_errors(&mut renderer, "import_dmabuf")?;
    let texture = renderer
        .import_dmabuf(&scene_target.dmabuf, None)
        .map_err(|err| anyhow!("import_dmabuf: {err}"))?;
    println!("  import_dmabuf: OK");
    // Side by side, same process, same moment, same fd: an independent EGL
    // display of joincheck's making, versus smithay's.
    {
        let mut raw = vec![0u8; (pixels * pixels * 4) as usize];
        let node_c = CString::new(node.as_str())?;
        let rc = unsafe {
            join_readback(node_c.as_ptr(), fd, pixels, pixels, stride, modifier, fourcc, raw.as_mut_ptr())
        };
        let nonzero = raw.iter().filter(|b| **b != 0).count();
        println!("  INDEPENDENT display reading the same fd: rc={rc}, {nonzero} of {} bytes non-zero", raw.len());
        // Checked, not printed. This is the only control separating "the
        // compositor cannot see these pixels" from "there are no pixels", and a
        // printed rc=-1 on a box whose render node enumerates differently
        // degrades it to nothing while the run still exits 0.
        if rc != 0 {
            return Err(anyhow!("the independent readback failed with rc={rc}"));
        }
        if nonzero == 0 {
            return Err(anyhow!(
                "an independent EGL display sees nothing in the buffer Qt reported rendering"
            ));
        }
        // That call left *its* context current. Take the thread back.
        restore(&renderer)?;
    }

    let gl_error = renderer
        .with_context(|gl| unsafe { gl.GetError() })
        .map_err(|err| anyhow!("with_context: {err}"))?;
    println!("  glGetError right after import_dmabuf: 0x{gl_error:x}");
    {
        use smithay::backend::renderer::Texture as _;
        let format = scene_target.dmabuf.format();
        let renderable = renderer.egl_context().dmabuf_render_formats().contains(&format);
        let samplable = renderer.egl_context().dmabuf_texture_formats().contains(&format);
        println!(
            "  texture {:?}; format {:?} mod 0x{:016x}: in render formats {renderable}, in texture formats {samplable} (external = {})",
            texture.size(), format.code, u64::from(format.modifier), !renderable
        );
    }
    // What smithay itself sees in the buffer, with no draw in between: bind the
    // imported texture as a framebuffer and copy it straight back.
    {
        let mut direct = texture.clone();
        let framebuffer = renderer
            .bind(&mut direct)
            .map_err(|err| anyhow!("binding the imported texture: {err}"))?;
        let mapping = renderer
            .copy_framebuffer(&framebuffer, Rectangle::from_size((pixels, pixels).into()), Fourcc::Argb8888)
            .map_err(|err| anyhow!("copying the imported texture: {err}"))?;
        drop(framebuffer);
        let raw = renderer.map_texture(&mapping).map_err(|err| anyhow!("mapping: {err}"))?.to_vec();
        let nonzero = raw.iter().filter(|b| **b != 0).count();
        println!("  straight read of the imported texture: {nonzero} of {} bytes non-zero", raw.len());
        let at = |x: i32, y: i32| {
            let i = ((y * pixels + x) * 4) as usize;
            [raw[i], raw[i + 1], raw[i + 2], raw[i + 3]]
        };
        println!("    corners bgra: tl {:?} tr {:?} bl {:?} br {:?}",
            at(pixels/4, pixels/4), at(3*pixels/4, pixels/4),
            at(pixels/4, 3*pixels/4), at(3*pixels/4, 3*pixels/4));
    }

    let got_element = element_for(&renderer, texture, (pixels, pixels), (logical, logical), scale);
    let got = draw_and_read(&mut renderer, &got_element, pixels, pixels, scale)?;

    for (name, buf) in [("reference", &reference), ("gpu", &got)] {
        let at = |x: i32, y: i32| {
            let i = ((y * pixels + x) * 4) as usize;
            [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
        };
        println!(
            "  {name:>9} corners bgra: tl {:?} tr {:?} bl {:?} br {:?}  ({} non-zero bytes)",
            at(pixels/4, pixels/4), at(3*pixels/4, pixels/4),
            at(pixels/4, 3*pixels/4), at(3*pixels/4, 3*pixels/4),
            buf.iter().filter(|b| **b != 0).count()
        );
    }
    let (bad, first) = differing(&reference, &got);
    println!(
        "\n  GPU path vs software path, byte for byte: {bad} of {} bytes differ",
        reference.len()
    );
    if let Some(i) = first {
        let px = i / 4;
        println!(
            "  first difference at byte {i} (pixel {}, {}): reference {:?} got {:?}",
            px % pixels as usize,
            px / pixels as usize,
            &reference[i - i % 4..i - i % 4 + 4],
            &got[i - i % 4..i - i % 4 + 4],
        );
    } else {
        println!("  the two paths produce the identical frame");
    }

    // ------------------------------------------------------------------
    // The pointer's route: Qt's dmabuf, read back and uploaded as memory.
    //
    // The one scene that does *not* reach the screen as a texture. smithay
    // reaches a DRM cursor plane only through `RenderElement::underlying_storage`,
    // whose two variants are `Wayland` and `Memory`; a `TextureRenderElement`
    // has none, so a GPU pointer drawn as a texture silently loses the plane and
    // every pointer motion becomes a full composite and page flip. So `cursor.rs`
    // lets Qt draw into the dmabuf -- it must, a GPU host refuses software scenes
    // -- and then reads it straight back into a `MemoryRenderBuffer`.
    //
    // That hangs on a claim nobody had checked: that what `copy_framebuffer`
    // hands back is byte-identical to what `import_memory` would have been given
    // on the software path. Three conventions meet there -- Qt's render target,
    // which `mirror_for_the_compositor` flips; smithay's readback; and
    // `MemoryRenderBuffer`'s top-down rows -- and two of them cancelling is not
    // the same as all three agreeing. Asserted twice: against the known picture
    // directly, and then through an element built from it.
    println!("\n=== the pointer's route: the dmabuf read back and uploaded as memory ===");
    {
        // The first frame's picture, which is the one every configuration of
        // this harness draws correctly -- placed here rather than after the
        // frame loop for that reason. Reading a buffer the loop above had
        // already found wrong would make this case fail on the loop's defect
        // and name it as a readback fault.
        let raw = read_dmabuf(&mut renderer, &scene_target.dmabuf, pixels, pixels)?;
        let want = expected_argb(pixels, pixels);
        let (bad, first) = differing(&want, &raw);
        println!(
            "  the readback vs the known picture, byte for byte: {bad} of {} bytes differ",
            want.len()
        );
        // An *empty* buffer is not this case's defect and must not be reported
        // as one. It means nothing wrote into the dmabuf, which is a render-path
        // failure -- the render control produces exactly that -- and a message
        // here about flipped or swizzled bytes would send the reader to the one
        // place the fault is not. The same misattribution the glGetError probes
        // had, in a different instrument.
        if bad != 0 && raw.iter().all(|byte| *byte == 0) {
            return Err(anyhow!(
                "the scene's buffer is empty: nothing has written into it, so there is nothing \
                 for the pointer's route to read back. That is a render-path failure and not a \
                 readback one -- read the frame comparison above this line, not this message"
            ));
        }
        if bad != 0 {
            let i = first.unwrap_or(0);
            return Err(anyhow!(
                "reading a scene's dmabuf back does not give the bytes the software path \
                 uploads: {bad} of {} bytes differ, first at {i} (want {:?} got {:?}) -- so a \
                 pointer read back this way is flipped, swizzled or padded",
                want.len(),
                &want[i - i % 4..i - i % 4 + 4],
                &raw[i - i % 4..i - i % 4 + 4],
            ));
        }

        // And through the element, which is what actually reaches the screen.
        let uploaded = renderer
            .import_memory(&raw, Fourcc::Argb8888, (pixels, pixels).into(), false)
            .map_err(|err| anyhow!("import_memory on the readback: {err}"))?;
        let element = element_for(&renderer, uploaded, (pixels, pixels), (logical, logical), scale);
        let drawn = draw_and_read(&mut renderer, &element, pixels, pixels, scale)?;
        let (bad, _) = differing(&reference, &drawn);
        println!("  and drawn through an element: {bad} of {} bytes differ", reference.len());
        if bad != 0 {
            return Err(anyhow!(
                "the pointer's readback route does not draw what the software path draws: \
                 {bad} of {} bytes differ",
                reference.len()
            ));
        }

        // What it costs, because the whole argument for this route is that the
        // pointer pays it once per size per change rather than once per frame.
        // A number here is what stops that being a guess: if it were milliseconds
        // the cache would not be enough and the design would need revisiting.
        const RUNS: u32 = 50;
        let started = std::time::Instant::now();
        for _ in 0..RUNS {
            let _ = read_dmabuf(&mut renderer, &scene_target.dmabuf, pixels, pixels)?;
        }
        let each = started.elapsed() / RUNS;
        println!("  import + bind + copy_framebuffer + map, {pixels}x{pixels}: {each:?} each");
    }

    // Frames 2..N: the steady state, and the only ordering the compositor is
    // ever actually in. Our context is current on the way in, because the last
    // frame put it back; Qt has to notice and take the thread again.
    let frames: usize = std::env::var("WIRECHECK_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut worst = 0usize;
    for frame in 2..=frames {
        // Wipe the buffer first, and this is the whole reason the comparison
        // below means anything.
        //
        // `quadrants.qml` paints an unchanging picture, and a frame Qt issues
        // against the *compositor's* context writes nothing at all -- so
        // without this the dmabuf still holds the previous frame's identical
        // pixels and the comparison reads zero. "Qt did not write" and "Qt
        // wrote the same thing again" are the same measurement. That is exactly
        // how this control came to pass with the render-path fix reverted.
        //
        // Cleared through the compositor's own renderer, which is also a small
        // proof in itself: if these pixels survive to the comparison, nothing
        // wrote over them.
        wipe(&mut renderer, &scene_target.dmabuf, pixels, pixels)?;

        poke(scene, pixels, scale);
        let mut fd2: c_int = -1;
        let rendered = unsafe { solium_qml_scene_render_gpu(scene, &raw mut fd2) };
        restore(&renderer)?;
        if rendered != 1 {
            return Err(anyhow!("frame {frame}: render_gpu returned {rendered}"));
        }
        if fd2 >= 0 {
            wait_for(&mut renderer, unsafe { OwnedFd::from_raw_fd(fd2) })?;
        }
        let texture = renderer
            .import_dmabuf(&scene_target.dmabuf, None)
            .map_err(|err| anyhow!("frame {frame}: import_dmabuf: {err}"))?;
        let element =
            element_for(&renderer, texture, (pixels, pixels), (logical, logical), scale);
        let got = draw_and_read(&mut renderer, &element, pixels, pixels, scale)?;
        let (bad, _) = differing(&reference, &got);
        worst = worst.max(bad);
        println!("  frame {frame}: {bad} of {} bytes differ from the reference", reference.len());
    }

    // ------------------------------------------------------------------
    // A resize that keeps the object tree.
    //
    // `render_on_gpu` used to answer a size change by rebuilding the scene --
    // a fresh QQuickRenderControl, a fresh QOpenGLContext, a fresh QRhi, a
    // fresh GBM allocation, a recompiled QML tree and, since C-1, a full Qt
    // teardown as well. The cost was the smaller half of it. A rebuilt scene is
    // a *new object tree*, so every animation, transition and stored property
    // inside it restarts from zero -- and a pane's scene is sized from an
    // *animating* rectangle, so that happened on every frame of every window
    // animation. A scene that animates while its window animates did not run
    // slowly; it never advanced.
    //
    // Two assertions, and neither is worth having alone. The counter carrying
    // across says the object tree survived. The picture being right at the new
    // size says the scene is drawing into the *new* buffer -- a rebind that
    // returned true and left Qt on the old texture would carry the counter
    // across perfectly.
    println!("\n=== a GPU scene resized onto a new buffer, in place ===");
    {
        let big_logical = logical * 2;
        let big_pixels = pixels * 2;
        let small =
            target::allocate(&gbm, pixels, pixels).context("the resize case's first buffer")?;
        let (fd4, stride4, modifier4, fourcc4) = small.as_ffi().context("as_ffi")?;
        let mut counting = unsafe {
            solium_qml_scene_new_gpu(
                qml.as_ptr(),
                pixels,
                pixels,
                fd4,
                stride4,
                modifier4,
                fourcc4,
                std::ptr::null(),
            )
        };
        if counting.is_null() {
            return Err(anyhow!("the resize case's scene would not build"));
        }
        restore(&renderer)?;
        unsafe { solium_qml_scene_resize(counting, pixels, pixels, scale) };

        for frame in 1..=5 {
            bump(counting);
            // The clock, as `render::prepare` advances it: once per frame, for
            // the whole process. Without it the animation below stands still
            // and proves nothing either way.
            tick(&mut clock, FRAME_MS);
            poke(counting, pixels, scale);
            let mut fd: c_int = -1;
            let rendered = unsafe { solium_qml_scene_render_gpu(counting, &raw mut fd) };
            restore(&renderer)?;
            if rendered != 1 {
                return Err(anyhow!(
                    "the resize case's frame {frame}: render_gpu returned {rendered}"
                ));
            }
            if fd >= 0 {
                wait_for(&mut renderer, unsafe { OwnedFd::from_raw_fd(fd) })?;
            }
        }
        let before = frames_of(counting);
        let spun_before = spin_of(counting);
        println!(
            "  {pixels}x{pixels}, five frames rendered; the tree's counter reads {before}, \
             its animation reads {spun_before}"
        );
        // The instrument, checked before what it measures. A `frames` property
        // that did not exist on the root item would read 0 every time and be
        // written 1 every time, and the comparison below would then fail for a
        // reason with nothing to do with resizing.
        if before != 5 {
            return Err(anyhow!(
                "the counter did not count: five frames left it at {before}, so this case \
                 cannot tell a rebuilt tree from a kept one"
            ));
        }
        // And the same question of the animation, which has its own ways to be
        // blind: a `spin` property that was not animated, or an animation the
        // driver never advanced because nothing ticked, both read 0 forever --
        // and 0 is a number every later reading is trivially greater than or
        // equal to. Asserted here so "it moved across the rebind" is a claim
        // about an animation that was running in the first place.
        if spun_before <= 0 {
            return Err(anyhow!(
                "the animation did not run: five ticked frames left `spin` at {spun_before}, \
                 so this case cannot tell a restarted animation from a continuing one"
            ));
        }
        // And the host's own answer about the same animation, which is what the
        // compositor actually gates its frames on. `spin` moving says an
        // animation ran; this says the scene will *admit* to it on a tick that
        // moved no pixel, which is the only tick where the answer decides
        // anything. See `animating` for why the pair of readings here and in
        // the scene case below is this instrument's negative control.
        if !animating(counting) {
            return Err(anyhow!(
                "`solium_qml_scene_animating` says nothing is animating in a scene whose \
                 `spin` just moved {spun_before} units under an `Animation.Infinite`. Every \
                 QML animation in the compositor stops dead one tick after it stops changing \
                 pixels if this is wrong"
            ));
        }

        // The new buffer, wiped through the compositor's own renderer before Qt
        // is asked for a frame in it -- the same reason the frame loop wipes. A
        // fresh GBM allocation is not reliably zeroed, and "Qt drew this" must
        // not be the same measurement as "this is what the allocator handed
        // back".
        let large = target::allocate(&gbm, big_pixels, big_pixels)
            .context("the resize case's second buffer")?;
        wipe(&mut renderer, &large.dmabuf, big_pixels, big_pixels)?;
        let (fd5, stride5, modifier5, fourcc5) = large.as_ffi().context("as_ffi")?;

        // The negative control for this case, kept rather than run once and
        // thrown away. With WIRECHECK_REBUILD_ON_RESIZE set, the resize is
        // answered the way `render_on_gpu` used to answer it -- a new scene on
        // the new buffer, the old one freed after it exists -- and the counter
        // check below must then fail. A check that passes either way proves
        // nothing, and this harness has been in that state four times.
        //
        // It has to live here rather than in `surface.rs`. Nothing in this
        // binary links the compositor crate, so reverting `render_on_gpu` to
        // `build(...)` changes nothing that this runs; the control has to be at
        // the level the harness actually measures, which is the host entry
        // point. What `surface.rs` still owns is the *choice* between the two,
        // and that is one line under a size comparison.
        drain_gl_errors(&mut renderer, "the rebind")?;
        let rebuild = std::env::var_os("WIRECHECK_REBUILD_ON_RESIZE").is_some();
        let ok = if rebuild {
            println!("  !! WIRECHECK_REBUILD_ON_RESIZE: rebuilding the scene instead of rebinding");
            let replacement = unsafe {
                solium_qml_scene_new_gpu(
                    qml.as_ptr(),
                    big_pixels,
                    big_pixels,
                    fd5,
                    stride5,
                    modifier5,
                    fourcc5,
                    std::ptr::null(),
                )
            };
            if replacement.is_null() {
                return Err(anyhow!("the control's replacement scene would not build"));
            }
            unsafe { solium_qml_scene_free(counting) };
            counting = replacement;
            unsafe { solium_qml_scene_resize(counting, big_pixels, big_pixels, scale) };
            true
        } else {
            unsafe {
                solium_qml_scene_rebind(
                    counting, fd5, stride5, modifier5, fourcc5, big_pixels, big_pixels, scale,
                )
            }
        };
        restore(&renderer)?;
        if !ok {
            return Err(anyhow!(
                "solium_qml_scene_rebind refused a {big_pixels}x{big_pixels} buffer"
            ));
        }
        // A rebind releases the previous EGLImage and the previous texture, and
        // this is the probe for what that left behind. It cannot prove the
        // releases went to the right context -- a GL delete against the wrong
        // one destroys whatever that context calls N and returns cleanly, which
        // is the whole reason the census below exists. What it does catch is a
        // delete that actually faulted, which would otherwise sit in the queue
        // until the teardown probe at the end of the run and be read as Qt's.
        let gl_error = renderer
            .with_context(|gl| unsafe { gl.GetError() })
            .map_err(|err| anyhow!("with_context after the rebind: {err}"))?;
        println!(
            "  {} onto a {big_pixels}x{big_pixels} buffer; glGetError after it: 0x{gl_error:x}",
            if rebuild { "rebuilt" } else { "rebound" }
        );
        if gl_error != 0 {
            return Err(anyhow!(
                "the resize left GL error 0x{gl_error:x} in the compositor's context"
            ));
        }

        // No poke: a rebind leaves the scene dirty by itself, having changed
        // both the geometry and the target.
        bump(counting);
        // The animation check's own negative control, and it needs one for the
        // reason every instrument here needs one: `spin` is read out of QML, and
        // a property that never moves reads the same number twice whether the
        // tree survived or not. With the clock stopped from here on, the tree is
        // kept and the counter still counts -- so the counter check below passes
        // -- and the animation assertion must fail on its own. A run where it
        // does not is a run where it was never testing anything.
        let ticked = std::env::var_os("WIRECHECK_STOP_THE_CLOCK").is_none();
        if ticked {
            tick(&mut clock, FRAME_MS);
        } else {
            println!(
                "  !! WIRECHECK_STOP_THE_CLOCK: not ticking past the rebind, so the animation \
                 assertion below must fail by itself"
            );
        }
        let mut fd6: c_int = -1;
        let rendered = unsafe { solium_qml_scene_render_gpu(counting, &raw mut fd6) };
        restore(&renderer)?;
        if rendered != 1 {
            return Err(anyhow!(
                "the frame after the rebind: render_gpu returned {rendered}"
            ));
        }
        if fd6 >= 0 {
            wait_for(&mut renderer, unsafe { OwnedFd::from_raw_fd(fd6) })?;
        }
        let after = frames_of(counting);
        let spun_after = spin_of(counting);
        println!(
            "  after the resize and one more frame, the counter reads {after} and the \
             animation reads {spun_after}"
        );
        if after <= before {
            return Err(anyhow!(
                "the QML tree was rebuilt by a resize: frames went {before} -> {after}, so \
                 every animation in a resizing scene restarts on every frame it is resized \
                 (and this one did: `spin` went {spun_before} -> {spun_after})"
            ));
        }
        // The counter is a proxy; this is the thing itself. A tree that
        // survived with every animation reset to its `from:` would carry the
        // counter across perfectly and still be the bug -- a decoration is
        // sized from an animating rectangle for the length of every window
        // animation, so "the tree was kept" is only worth anything if what was
        // running in it kept running.
        if spun_after <= spun_before {
            // Two whole messages rather than one with a clause spliced into it,
            // because they are two different findings: under the control this
            // is the instrument working, and saying "across a ticked frame"
            // when the frame was deliberately not ticked is the harness lying
            // about its own run.
            return Err(if ticked {
                anyhow!(
                    "a running animation did not survive the resize: `spin` went \
                     {spun_before} -> {spun_after} across a rebind and a ticked frame, so an \
                     animation inside a resizing scene stands still"
                )
            } else {
                anyhow!(
                    "`spin` went {spun_before} -> {spun_after} across a rebind and a frame the \
                     clock was deliberately stopped for. This is WIRECHECK_STOP_THE_CLOCK, the \
                     animation assertion's own negative control: failing here is what it is \
                     for, and the counter passing above it is the other half"
                )
            });
        }

        // And it is drawing the new buffer, at the new size. Through the same
        // comparison the rest of this harness uses rather than an absolute
        // orientation: the identical picture uploaded with `import_memory` and
        // drawn through identical element parameters, so whatever convention
        // the offscreen target has cancels.
        let big_reference_texture = renderer
            .import_memory(
                &expected_argb(big_pixels, big_pixels),
                Fourcc::Argb8888,
                (big_pixels, big_pixels).into(),
                false,
            )
            .map_err(|err| anyhow!("the resized reference: {err}"))?;
        let big_reference_element = element_for(
            &renderer,
            big_reference_texture,
            (big_pixels, big_pixels),
            (big_logical, big_logical),
            scale,
        );
        let big_reference = draw_and_read(
            &mut renderer,
            &big_reference_element,
            big_pixels,
            big_pixels,
            scale,
        )?;
        let big_texture = renderer
            .import_dmabuf(&large.dmabuf, None)
            .map_err(|err| anyhow!("import_dmabuf on the resized buffer: {err}"))?;
        let big_element = element_for(
            &renderer,
            big_texture,
            (big_pixels, big_pixels),
            (big_logical, big_logical),
            scale,
        );
        let big_got =
            draw_and_read(&mut renderer, &big_element, big_pixels, big_pixels, scale)?;
        let (big_bad, _) = differing(&big_reference, &big_got);
        println!(
            "  the resized picture vs the software path: {big_bad} of {} bytes differ",
            big_reference.len()
        );
        if big_bad != 0 {
            return Err(anyhow!(
                "a scene rebound onto a {big_pixels}x{big_pixels} buffer does not draw it: \
                 {big_bad} of {} bytes differ from the software path",
                big_reference.len()
            ));
        }

        // Freed in the dangerous ordering, with the compositor's context
        // current, and censused either side of it. Not for its own sake -- C-1
        // below is the case for that -- but so C-1 starts from an undamaged
        // baseline: its census is taken after this block, and anything wrecked
        // here would be invisible to it.
        //
        // Which is exactly why it has to be skippable. This free is C-1's own
        // ordering, so under the teardown control it reaches the same defect
        // first and the run stops here -- and then C-1's census, its
        // precondition assertion and its guard textures only ever execute in
        // the *passing* state, where a regression in the instrument itself
        // would be invisible. WIRECHECK_KEEP_RESIZED_SCENE leaves this scene
        // alive to the end of the process so that control reaches C-1. Nothing
        // is leaked past the run, and Qt keeps its own reference to the buffer.
        if std::env::var_os("WIRECHECK_KEEP_RESIZED_SCENE").is_some() {
            println!(
                "  !! WIRECHECK_KEEP_RESIZED_SCENE: left alive, so C-1 below runs its own \
                 instrument under the teardown control"
            );
        } else {
            restore(&renderer)?;
            let live_before_free = gl_names(&mut renderer, 64)?;
            unsafe { solium_qml_scene_free(counting) };
            restore(&renderer)?;
            let live_after_free = gl_names(&mut renderer, 64)?;
            let lost: Vec<_> = live_before_free
                .iter()
                .filter(|it| !live_after_free.contains(it))
                .collect();
            println!("  DESTROYED in our context by freeing the resized scene: {lost:?}");
            if !lost.is_empty() {
                return Err(anyhow!(
                    "freeing the resized scene destroyed {} of the compositor's GL objects",
                    lost.len()
                ));
            }
        }
    }

    // ------------------------------------------------------------------
    // Every scene the compositor builds, on a GPU host.
    //
    // Before Task 7 the cursor and the window frames went down
    // `solium_qml_scene_new_with`, which a GPU host refuses outright -- one
    // scene graph per process, and Qt picked the other one. So `SOLIUM_QML_GPU=1`
    // produced a desktop with a wallpaper on it and no window frames and no
    // pointer, each refusal logged by its own caller as its own unrelated
    // failure and nothing anywhere saying that a whole class of scene was
    // missing.
    //
    // The compositor's *real* QML, not a stand-in: what this is checking is
    // that these particular files come up under the RHI scene graph, and a
    // `Rectangle` of our own would come up under anything. `cursor.qml` draws
    // through `QtQuick.Shapes` with the curve renderer and `top.qml` lays out
    // text and an animated `Behavior`, neither of which the four flat rectangles
    // in `quadrants.qml` exercise at all.
    //
    // What it cannot do is check the picture: there is no reference for a
    // titlebar here and inventing one would be asserting today's design system
    // rather than the path. So it wipes the buffer through the compositor's own
    // renderer first and asks whether anything at all came back -- which does
    // prove Qt built the component, brought up an RHI for it, imported *our*
    // dmabuf and wrote into it, and proves nothing whatever about what it drew.
    println!("\n=== every scene the compositor builds, on a GPU host ===");
    // Kept alive to the end of the run rather than freed, and deliberately.
    //
    // A free with the compositor's context current is C-1's own ordering, so
    // freeing anything here would reach that defect *before* C-1's census is
    // taken -- and under the teardown control, with WIRECHECK_KEEP_RESIZED_SCENE
    // set to let C-1 run its own instrument, this would stop the run in the
    // resize case's place and C-1 would go back to only ever executing in the
    // passing state. Nothing is leaked past the run; Qt keeps its own reference
    // to each buffer, and the process is about to exit.
    let mut kept_scenes: Vec<*mut c_void> = Vec::new();
    // The buffers with them: a `Target` closes its dmabuf fd when it drops, and
    // a scene that is still alive is still pointed at one.
    let mut kept_buffers: Vec<target::Target> = Vec::new();
    for (what, file, w, h) in [
        ("cursor", "crates/solium/qml/cursor.qml", 64, 64),
        ("decoration", "crates/solium/qml/decorations/top.qml", 640, 480),
    ] {
        let path = CString::new(repo().join(file).as_os_str().as_encoded_bytes())?;
        let buffer = target::allocate(&gbm, w, h)
            .with_context(|| format!("the {what} scene's buffer"))?;
        wipe(&mut renderer, &buffer.dmabuf, w, h)?;
        let (fd, stride, modifier, fourcc) = buffer.as_ffi().context("as_ffi")?;
        let built = unsafe {
            solium_qml_scene_new_gpu(
                path.as_ptr(),
                w,
                h,
                fd,
                stride,
                modifier,
                fourcc,
                std::ptr::null(),
            )
        };
        restore(&renderer)?;
        if built.is_null() {
            return Err(anyhow!(
                "a GPU host could not build the {what} scene ({file}); with the compositor's \
                 own QML this is the refusal at host.cpp's software constructor, which is what \
                 a desktop with no frames and no pointer looks like from in here"
            ));
        }
        unsafe { solium_qml_scene_resize(built, w, h, scale) };
        tick(&mut clock, FRAME_MS);
        let mut fd7: c_int = -1;
        let rendered = unsafe { solium_qml_scene_render_gpu(built, &raw mut fd7) };
        restore(&renderer)?;
        if rendered != 1 {
            return Err(anyhow!(
                "the {what} scene returned {rendered} from render_gpu"
            ));
        }
        if fd7 >= 0 {
            wait_for(&mut renderer, unsafe { OwnedFd::from_raw_fd(fd7) })?;
        }
        let raw = read_dmabuf(&mut renderer, &buffer.dmabuf, w, h)?;
        let nonzero = raw.iter().filter(|byte| **byte != 0).count();
        println!(
            "  {what} ({w}x{h}, {file}): built, rendered, {nonzero} of {} bytes non-zero",
            raw.len()
        );
        if nonzero == 0 {
            return Err(anyhow!(
                "the {what} scene rendered into a buffer this wiped first and left it empty"
            ));
        }
        // The other half of the animation census, and the half that makes it an
        // instrument rather than a rubber stamp. Neither of these two has been
        // written to, so neither has a `Behavior` to have started and nothing
        // in either is animating -- `cursor.qml` has no animation in it at all.
        // They must read 0 here while `quadrants.qml` reads 1 above, in the
        // same process, with the same driver, on the same tick budget.
        //
        // Which is also what rules out answering this from `QAnimationDriver`:
        // it is one object for the process, `quadrants.qml` has been animating
        // since long before this loop, and Qt restarts a hand-advanced driver
        // on every tick whether or not anything is registered. A process-wide
        // answer is `true` on this line, for ever, and a compositor gated on it
        // never sleeps again.
        if animating(built) {
            return Err(anyhow!(
                "`solium_qml_scene_animating` says the {what} scene ({file}) is animating, \
                 and nothing has written a property on it or declared an animation in it. \
                 An answer that is always yes keeps every monitor redrawing at full rate \
                 for as long as the compositor is running"
            ));
        }
        kept_scenes.push(built);
        kept_buffers.push(buffer);
    }

    // ------------------------------------------------------------------
    // The first rebind, on a scene that has never rendered.
    //
    // The shape production is actually in, and the one the resize case above
    // cannot reach: it renders five frames before it rebinds. Nothing on screen
    // does. `ShellSurface::new` builds at 1x1 with its size recorded as (0, 0),
    // because the real size is not known until something asks for a frame; a
    // `Decoration` is built at the client's size and has to end up on the
    // *outer* rect at the monitor's scale, which is two numbers that arrive
    // with the first draw. Both take the rebind branch on their very first
    // frame, with `QQuickRenderControl::initialize()` the only thing that has
    // ever made this scene's context current.
    //
    // Worth its own case because the rebind's preconditions are not obviously
    // met there. `take_the_thread` needs the scene's `qt_context` and
    // `qt_surface`, and `clear_stale_current_context` is reasoning about a
    // thread-local Qt sets when it renders -- on this path it has never
    // rendered, so that belief is either null or another scene's.
    println!("\n=== the first rebind, on a scene that has never rendered ===");
    {
        let placeholder = target::allocate(&gbm, 1, 1).context("the 1x1 placeholder buffer")?;
        let (fd8, stride8, modifier8, fourcc8) = placeholder.as_ffi().context("as_ffi")?;
        let fresh = unsafe {
            solium_qml_scene_new_gpu(
                qml.as_ptr(),
                1,
                1,
                fd8,
                stride8,
                modifier8,
                fourcc8,
                std::ptr::null(),
            )
        };
        restore(&renderer)?;
        if fresh.is_null() {
            return Err(anyhow!("the 1x1 scene would not build"));
        }
        println!("  built 1x1; no render, no resize, straight to the rebind");

        let real = target::allocate(&gbm, pixels, pixels).context("the first real buffer")?;
        // Wiped for the same reason every other case here wipes: a fresh GBM
        // allocation is not reliably zeroed, and "Qt drew this" must not be the
        // same measurement as "this is what the allocator handed back".
        wipe(&mut renderer, &real.dmabuf, pixels, pixels)?;
        let (fd9, stride9, modifier9, fourcc9) = real.as_ffi().context("as_ffi")?;
        drain_gl_errors(&mut renderer, "the first rebind")?;
        let ok = unsafe {
            solium_qml_scene_rebind(
                fresh, fd9, stride9, modifier9, fourcc9, pixels, pixels, scale,
            )
        };
        restore(&renderer)?;
        if !ok {
            return Err(anyhow!(
                "a scene that has never rendered would not rebind from 1x1 onto \
                 {pixels}x{pixels} -- which is the first frame of every shell surface and \
                 every window frame on this path"
            ));
        }
        // The same probe the resize case takes after its rebind, and with the
        // same limit: it cannot prove the releases went to the right context --
        // a GL delete against the wrong one destroys whatever that context calls
        // N and returns cleanly, which is what the censuses below are for. What
        // it catches is a delete that actually faulted, which would otherwise
        // sit in the queue until the teardown probe at the end and be read as
        // Qt's.
        let gl_error = renderer
            .with_context(|gl| unsafe { gl.GetError() })
            .map_err(|err| anyhow!("with_context after the first rebind: {err}"))?;
        println!("  rebound onto {pixels}x{pixels}; glGetError after it: 0x{gl_error:x}");
        if gl_error != 0 {
            return Err(anyhow!(
                "the first rebind left GL error 0x{gl_error:x} in the compositor's context"
            ));
        }

        tick(&mut clock, FRAME_MS);
        let mut fd10: c_int = -1;
        let rendered = unsafe { solium_qml_scene_render_gpu(fresh, &raw mut fd10) };
        restore(&renderer)?;
        if rendered != 1 {
            return Err(anyhow!(
                "the first frame after a never-rendered scene's rebind: render_gpu returned \
                 {rendered}"
            ));
        }
        if fd10 >= 0 {
            wait_for(&mut renderer, unsafe { OwnedFd::from_raw_fd(fd10) })?;
        }
        // And it is the *new* buffer being drawn, at the new size. Through the
        // same reference comparison as everything else here rather than an
        // absolute orientation: a rebind that returned true and left Qt on the
        // 1x1 texture would sail past a non-zero check.
        let texture = renderer
            .import_dmabuf(&real.dmabuf, None)
            .map_err(|err| anyhow!("import_dmabuf on the first rebind's buffer: {err}"))?;
        let element = element_for(&renderer, texture, (pixels, pixels), (logical, logical), scale);
        let got = draw_and_read(&mut renderer, &element, pixels, pixels, scale)?;
        let (bad, _) = differing(&reference, &got);
        println!("  the picture after it vs the software path: {bad} of {} bytes differ", reference.len());
        if bad != 0 {
            return Err(anyhow!(
                "a scene rebound before it had ever rendered does not draw its new buffer: \
                 {bad} of {} bytes differ from the software path",
                reference.len()
            ));
        }
        // Kept alive, for the reason the case above states.
        kept_scenes.push(fresh);
        kept_buffers.push(real);
        kept_buffers.push(placeholder);
    }
    println!(
        "  {} scene(s) and {} buffer(s) left alive so C-1 below keeps its baseline",
        kept_scenes.len(),
        kept_buffers.len()
    );

    // ------------------------------------------------------------------
    // Built and freed without ever being rendered.
    //
    // The free path that reaches `clear_stale_current_context`'s
    // `believed == nullptr` early return: `release_the_thread` left the thread
    // empty when this scene was built, and Qt's thread-local was never set
    // because nothing rendered. The branch is correct -- a null thread-local
    // means Qt holds no stale belief, so `ensureContext()` will make its
    // context current properly on the way out -- but it is the one branch
    // neither the frame loop nor C-1 ever takes.
    //
    // It used to be the hot path by accident: a resize rebuilt the scene, so
    // every frame of every animation built one scene and freed another. It is
    // not any more, which is why this case has to exist deliberately. The
    // render side of the same branch still runs on its own -- a rebind gives
    // the thread back, so the render right after one finds a null belief -- and
    // that is the resize case above.
    println!("\n=== a scene built and freed without ever rendering ===");
    {
        let unrendered = target::allocate(&gbm, pixels, pixels).context("second buffer")?;
        let (fd2, stride2, modifier2, fourcc2) = unrendered.as_ffi().context("as_ffi")?;
        let before = gl_names(&mut renderer, 64)?;
        let scene2 = unsafe {
            solium_qml_scene_new_gpu(
                qml.as_ptr(),
                pixels,
                pixels,
                fd2,
                stride2,
                modifier2,
                fourcc2,
                std::ptr::null(),
            )
        };
        if scene2.is_null() {
            return Err(anyhow!("the second scene would not build"));
        }
        println!("  built; no render at all");
        // Put the compositor's context back before the free. It still takes
        // `clear_stale_current_context`'s null-belief early return -- nothing
        // rendered, so Qt has no belief -- but it puts the census either side
        // of a *live* compositor context, which is the one state no other case
        // here exercises.
        restore(&renderer)?;
        unsafe { solium_qml_scene_free(scene2) };
        restore(&renderer)?;
        let after = gl_names(&mut renderer, 64)?;
        let lost: Vec<_> = before.iter().filter(|it| !after.contains(it)).collect();
        println!("  DESTROYED in our context by build-then-free: {lost:?}");
        if !lost.is_empty() {
            return Err(anyhow!(
                "building and freeing a scene without rendering destroyed {} of the compositor's GL objects",
                lost.len()
            ));
        }
        // And the renderer still works afterwards, which is the other half of
        // "nothing was quietly taken".
        let still = renderer
            .import_memory(&expected_argb(4, 4), Fourcc::Argb8888, (4, 4).into(), false)
            .map(|_| "OK")
            .unwrap_or("FAILED");
        println!("  a GL call afterwards: {still}");
    }

    // ------------------------------------------------------------------
    // The ordering the rest of this harness structurally cannot produce.
    //
    // Both rebuild paths in `render_on_gpu` evaluate `build(...)` before the old
    // `Scene` is dropped, so at free time the *new* scene's context is current
    // and QRhi's ensureContext() does the right thing by accident. That is the
    // same shape as the pre-flight passing on frame one: the instrument
    // arranged so the bug cannot appear.
    //
    // The reachable ordering has no new scene in it at all -- a pane closing, a
    // loading scene replaced when its client attaches, a scripted instance
    // dropped on reload. Then the compositor's own context is what is current
    // when Qt tears down, QRhiGles2::destroy() and the scenegraph invalidate
    // skip their makeCurrent, and executeDeferredReleases() runs
    // glDeleteTextures / glDeleteBuffers / glDeleteFramebuffers /
    // glDeleteProgram for *Qt's* names against *our* objects.
    //
    // So: guard textures the compositor owns, read before and after, with the
    // free done in exactly that ordering.
    println!("\n=== C-1: free a scene with the compositor's context current ===");
    const GUARDS: usize = 8;
    let mut guards = Vec::new();
    for index in 0..GUARDS {
        let texture = renderer
            .import_memory(
                &guard_image(index, pixels, pixels),
                Fourcc::Argb8888,
                (pixels, pixels).into(),
                false,
            )
            .map_err(|err| anyhow!("guard texture {index}: {err}"))?;
        let element =
            element_for(&renderer, texture.clone(), (pixels, pixels), (logical, logical), scale);
        let before = draw_and_read(&mut renderer, &element, pixels, pixels, scale)?;
        guards.push((texture, before));
    }
    {
        use smithay::backend::renderer::gles::GlesTexture as _T;
        let names: Vec<u32> = guards.iter().map(|(t, _)| _T::tex_id(t)).collect();
        println!("  {GUARDS} guard textures the compositor owns, GL names {names:?}");
    }

    // Qt's own names, for the overlap that makes this dangerous at all.
    println!("  the scene's texture in Qt's context is GL name 1 (joincheck reports it)");

    // Our context is current here -- the frame loop above restored it -- and
    // nothing else is built. This is the ordering.
    let live_before = gl_names(&mut renderer, 64)?;
    println!("  GL objects live in the compositor's context before the free: {live_before:?}");

    // One more render, immediately before the free, so Qt's thread-local names
    // this scene's context and EGL names ours. That is the state the whole case
    // is about, and it has to be established here rather than inherited: the
    // build-and-free above tears a context down, and a context destructor
    // clears the thread-local. Getting that ordering by luck is how the first
    // version of this test came to pass with the bug present.
    poke(scene, pixels, scale);
    let mut fd3: c_int = -1;
    if unsafe { solium_qml_scene_render_gpu(scene, &raw mut fd3) } != 1 {
        return Err(anyhow!("the pre-free render failed"));
    }
    restore(&renderer)?;
    if fd3 >= 0 {
        wait_for(&mut renderer, unsafe { OwnedFd::from_raw_fd(fd3) })?;
    }

    // The precondition, asserted rather than assumed, and asserted about *this*
    // scene rather than about the existence of a belief.
    //
    // Both halves are needed. A belief naming some other live scene is safe --
    // ensureContext() compares it against its own ctx and corrects -- and a
    // belief that EGL agrees with is not stale at all. Only "Qt thinks it has
    // the thread, for this scene, and it does not" makes the teardown skip its
    // makeCurrent, which is the thing under test.
    let names = unsafe { wirecheck_belief_names_scene(scene) };
    let egl_agrees = unsafe { wirecheck_egl_agrees_with(scene) };
    println!(
        "  precondition: Qt's belief names this scene = {}, EGL agrees = {} (want true/false)",
        names == 1,
        egl_agrees == 1
    );
    if names != 1 || egl_agrees != 0 {
        return Err(anyhow!(
            "precondition lost: Qt's belief is not a stale one about this scene, \
             so this case cannot distinguish the fix from its absence"
        ));
    }

    println!("  freeing the scene with the compositor's context current");
    drain_gl_errors(&mut renderer, "Qt's teardown")?;
    unsafe { solium_qml_scene_free(scene) };
    restore(&renderer)?;

    let live_after = gl_names(&mut renderer, 64)?;
    let destroyed: Vec<_> = live_before
        .iter()
        .filter(|it| !live_after.contains(it))
        .collect();
    println!("  GL objects live after:  {live_after:?}");
    println!("  DESTROYED by Qt's teardown, in our context: {destroyed:?}");
    let ours: Vec<_> = destroyed
        .iter()
        .filter(|it| live_before_qt.contains(***&it))
        .collect();
    println!(
        "  ...of which existed before Qt was started, so are certainly ours: {ours:?}"
    );

    let mut clobbered = 0usize;
    for (index, (texture, before)) in guards.iter().enumerate() {
        let element =
            element_for(&renderer, texture.clone(), (pixels, pixels), (logical, logical), scale);
        let after = draw_and_read(&mut renderer, &element, pixels, pixels, scale)?;
        let (bad, _) = differing(before, &after);
        if bad != 0 {
            clobbered += 1;
        }
        println!(
            "    guard {index}: {bad} of {} bytes differ after Qt's teardown",
            before.len()
        );
    }

    let gl_error = renderer
        .with_context(|gl| unsafe { gl.GetError() })
        .map_err(|err| anyhow!("with_context after teardown: {err}"))?;
    println!("  glGetError after Qt's teardown: 0x{gl_error:x}");
    let still_renders = renderer
        .import_memory(&expected_argb(4, 4), Fourcc::Argb8888, (4, 4).into(), false)
        .map(|_| "OK")
        .unwrap_or("FAILED");
    println!("  a GL call after scene_free (context restored): {still_renders}");
    println!(
        "  => {} of {GUARDS} compositor textures survived Qt's teardown",
        GUARDS - clobbered
    );

    if clobbered != 0 || !destroyed.is_empty() {
        return Err(anyhow!(
            "Qt's teardown destroyed {} of the compositor's GL objects ({clobbered} of {GUARDS} guard textures visibly clobbered)",
            destroyed.len()
        ));
    }

    if bad != 0 || worst != 0 {
        return Err(anyhow!("the GPU path does not match the software path"));
    }
    Ok(())
}
