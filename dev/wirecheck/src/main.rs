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
    fn solium_qml_scene_free(scene: *mut c_void);

    fn wirecheck_belief_names_scene(scene: *mut c_void) -> c_int;
    fn wirecheck_egl_agrees_with(scene: *mut c_void) -> c_int;

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
        // Qt's warnings go to journald on Fedora otherwise, which is the whole
        // diagnostic half of host.cpp invisible. See dev/README.md.
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
fn wipe(renderer: &mut GlesRenderer, buffer: &smithay::backend::allocator::dmabuf::Dmabuf, side: i32) -> Result<()> {
    let mut buffer = buffer.clone();
    {
        let mut framebuffer = renderer
            .bind(&mut buffer)
            .map_err(|err| anyhow!("binding the scene buffer to wipe it: {err}"))?;
        let mut frame = renderer
            .render(&mut framebuffer, (side, side).into(), Transform::Normal)
            .map_err(|err| anyhow!("wiping: {err}"))?;
        frame
            .clear(Color32F::TRANSPARENT, &[Rectangle::from_size((side, side).into())])
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
        wipe(&mut renderer, &scene_target.dmabuf, pixels)?;

        // Make the scene dirty without changing what it lays out to: a scale
        // change and back leaves the same geometry and the same picture.
        unsafe { solium_qml_scene_resize(scene, pixels, pixels, scale * 2.0) };
        unsafe { solium_qml_scene_resize(scene, pixels, pixels, scale) };
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
    // Built and freed without ever being rendered.
    //
    // The path a resize rebuild takes for the scene it is replacing, and the
    // one that reaches `clear_stale_current_context`'s `believed == nullptr`
    // early return: `release_the_thread` left the thread empty when this scene
    // was built, and Qt's thread-local was never set because nothing rendered.
    // The branch is correct -- a null thread-local means Qt holds no stale
    // belief, so `ensureContext()` will make its context current properly on
    // the way out -- but neither of the two runs above ever takes it, and Task
    // 6 makes it run on every frame.
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
    unsafe { solium_qml_scene_resize(scene, pixels, pixels, scale * 2.0) };
    unsafe { solium_qml_scene_resize(scene, pixels, pixels, scale) };
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
