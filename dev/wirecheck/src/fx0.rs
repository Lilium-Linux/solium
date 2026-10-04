//! Phase 0 of the shader work, on a real GPU: the cases `cargo test` cannot
//! reach because they need a GL context. Run alone with `WIRECHECK_ONLY=fx0`,
//! and after case 11 in a full run. Each prints a `=== FX0:` heading.

use anyhow::{Result, anyhow};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, ffi};
use smithay::backend::renderer::{
    Bind as _, Color32F, ExportMem as _, Frame as _, Offscreen as _, Renderer as _,
};
use smithay::utils::{Rectangle, Transform};

#[path = "../../../crates/solium/src/gputime.rs"]
#[allow(dead_code, reason = "the compositor's timer, of which this case needs part")]
mod gputime;

/// Every FX0 case, in order.
pub(crate) fn all(renderer: &mut GlesRenderer) -> Result<()> {
    gpu_timestamps(renderer)?;
    closed_outside_a_frame(renderer)?;
    no_wait_through_smithay(renderer)?;
    no_wait_through_raw_gl(renderer)?;
    Ok(())
}

/// **Case 11b, second half: a region closed outside a frame resolves at the
/// first idle after its work is done.** The TTY's shape: `open` before a
/// frame and `close` after its `finish`, as `tty.rs` stamps an output around
/// `render_frame`; then the frame's fence waited on, as the kernel waits on
/// it before the flip, so the frame's work is done however slow the GPU
/// (iris at cold clocks); 10 ms for the flip; then one `idle`, as the vblank
/// handler calls it. The wait flushes nothing (`SyncPoint::wait`), so the
/// closing stamp reaches the GPU only if `close` sent it. Ten passes, and
/// every one must be in at that first idle, or the report waiting for it
/// goes as `unread` (`Timer::close`).
fn closed_outside_a_frame(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX0: a region closed outside a frame resolves at the first idle ===");
    let mut timer = gputime::Timer::new(renderer);
    let side = 1024;
    let mut target: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (side, side).into())
        .map_err(|err| anyhow!("a target for the timed draw: {err}"))?;
    let whole = Rectangle::from_size((side, side).into());
    let passes = 10_u64;
    let mut missing = Vec::new();
    for pass in 1..=passes {
        timer.begin_pass(renderer, pass);
        for _ in timer.take_resolved() {}
        let stamp = timer.open(renderer, gputime::Region::Output(0));
        let sync = {
            let mut framebuffer = renderer
                .bind(&mut target)
                .map_err(|err| anyhow!("binding: {err}"))?;
            let mut frame = renderer
                .render(&mut framebuffer, (side, side).into(), Transform::Normal)
                .map_err(|err| anyhow!("rendering: {err}"))?;
            for index in 0..50 {
                let shade = (index % 10) as f32 / 10.0;
                frame
                    .draw_solid(whole, &[whole], Color32F::new(shade, 0.2, 0.4, 1.0))
                    .map_err(|err| anyhow!("drawing: {err}"))?;
            }
            frame.finish().map_err(|err| anyhow!("finishing: {err}"))?
        };
        timer.close(renderer, stamp);
        // The kernel's wait before the flip, after the close as on the TTY.
        sync.wait()
            .map_err(|err| anyhow!("waiting for the frame's fence: {err}"))?;
        std::thread::sleep(std::time::Duration::from_millis(10));
        timer.idle(renderer);
        let resolved = timer
            .take_resolved()
            .any(|(each, gpu)| each == pass && matches!(gpu, gputime::Gpu::Ok(_)));
        if !resolved {
            missing.push(pass);
        }
    }
    if missing.is_empty() {
        println!("  {passes} of {passes} passes resolved at the first idle after their work");
        Ok(())
    } else {
        Err(anyhow!(
            "passes {missing:?} were not resolved at the first idle after their work: \
             a stamp made after a frame's finish is not flushed"
        ))
    }
}

/// The least GPU time 200 fills of 1024² can take: 200 Mpix at 2 Tpix/s,
/// several times the fill rate of any GPU there is. Two timestamps that do
/// not bracket the fills read a microsecond or two, and this is how case 11b
/// tells them apart (its control closes the region before the fills).
const LEAST_PLAUSIBLE_NS: u64 = 100_000;

/// **Case 11b: a pass's GPU time resolves, and is a plausible number.** 200
/// full-target fills into a 1024² texture, inside a frame, between two
/// timestamps: the timer must find the extension, load both entry points,
/// and resolve the pass within 20 polls of `idle`, 10 ms apart, with
/// `100 us <= ns < 1 s` ([`LEAST_PLAUSIBLE_NS`]): the stamps time the fills.
fn gpu_timestamps(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX0: GPU timestamps through GL_EXT_disjoint_timer_query ===");
    let mut timer = gputime::Timer::new(renderer);
    if !timer.supported() {
        return Err(anyhow!(
            "GL_EXT_disjoint_timer_query is not usable here, so gpu_us cannot be measured"
        ));
    }
    let side = 1024;
    let mut target: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (side, side).into())
        .map_err(|err| anyhow!("a target for the timed draw: {err}"))?;
    let whole = Rectangle::from_size((side, side).into());
    // One pass, drawn and finished once. Then only `idle` polls: it resolves
    // what is in without taking a slot, so pass 1's slot is never recycled
    // and its result cannot be dropped as late while a slow GPU (iris at cold
    // clocks) is still drawing it. Up to 20 polls 10 ms apart.
    timer.begin_pass(renderer, 1);
    {
        let mut framebuffer = renderer
            .bind(&mut target)
            .map_err(|err| anyhow!("binding: {err}"))?;
        let mut frame = renderer
            .render(&mut framebuffer, (side, side).into(), Transform::Normal)
            .map_err(|err| anyhow!("rendering: {err}"))?;
        let stamp = timer.open_in(&mut frame, gputime::Region::Capture);
        for index in 0..200 {
            let shade = (index % 10) as f32 / 10.0;
            frame
                .draw_solid(whole, &[whole], Color32F::new(shade, 0.2, 0.4, 1.0))
                .map_err(|err| anyhow!("drawing: {err}"))?;
        }
        timer.close_in(&mut frame, stamp);
        let _ = frame.finish().map_err(|err| anyhow!("finishing: {err}"))?;
    }
    let mut got = None;
    for _poll in 0..20 {
        timer.idle(renderer);
        for (resolved, gpu) in timer.take_resolved() {
            if resolved == 1 {
                got = Some(gpu);
            }
        }
        if got.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    match got {
        Some(gputime::Gpu::Ok(sample))
            if (LEAST_PLAUSIBLE_NS..1_000_000_000).contains(&sample.captures_ns) =>
        {
            println!(
                "  200 fills of 1024x1024 took {} us of GPU time",
                sample.captures_ns / 1_000
            );
            Ok(())
        }
        other => Err(anyhow!(
            "pass 1's GPU time did not resolve to a plausible number: {other:?}. \
             Under {LEAST_PLAUSIBLE_NS} ns means the two timestamps do not bracket \
             the fills drawn between them"
        )),
    }
}

/// The last fill's colour alternates per round, so a read that saw the
/// previous round, or an unfinished one, is told from a right one. White and
/// black read the same in either channel order.
fn shade(round: u32) -> ([f32; 3], u8) {
    if round % 2 == 0 {
        ([1.0, 1.0, 1.0], 255)
    } else {
        ([0.0, 0.0, 0.0], 0)
    }
}

/// 500 full-target fills of 1024², the last one `colour`, finished and the
/// fence **dropped unwaited**: what a capture with `SOLIUM_FENCE_WAIT=off` does.
fn draw_heavy(
    renderer: &mut GlesRenderer,
    source: &mut GlesTexture,
    colour: [f32; 3],
) -> Result<()> {
    let side = 1024;
    let whole = Rectangle::from_size((side, side).into());
    let mut framebuffer = renderer
        .bind(source)
        .map_err(|err| anyhow!("binding the capture: {err}"))?;
    let mut frame = renderer
        .render(&mut framebuffer, (side, side).into(), Transform::Normal)
        .map_err(|err| anyhow!("rendering the capture: {err}"))?;
    for _ in 0..499 {
        frame
            .draw_solid(whole, &[whole], Color32F::new(0.5, 0.5, 0.5, 1.0))
            .map_err(|err| anyhow!("{err}"))?;
    }
    frame
        .draw_solid(whole, &[whole], Color32F::new(colour[0], colour[1], colour[2], 1.0))
        .map_err(|err| anyhow!("{err}"))?;
    drop(frame.finish().map_err(|err| anyhow!("finishing the capture: {err}"))?);
    Ok(())
}

/// Read a texture back, after its own draw has been waited for.
fn read(renderer: &mut GlesRenderer, texture: &mut GlesTexture, side: i32) -> Result<Vec<u8>> {
    let framebuffer = renderer
        .bind(texture)
        .map_err(|err| anyhow!("binding to read: {err}"))?;
    let mapping = renderer
        .copy_framebuffer(&framebuffer, Rectangle::from_size((side, side).into()), Fourcc::Argb8888)
        .map_err(|err| anyhow!("copying: {err}"))?;
    drop(framebuffer);
    Ok(renderer
        .map_texture(&mapping)
        .map_err(|err| anyhow!("mapping: {err}"))?
        .to_vec())
}

/// Every colour byte is `value` (alpha is not looked at).
fn all_are(pixels: &[u8], value: u8) -> bool {
    pixels
        .chunks_exact(4)
        .all(|pixel| pixel[..3].iter().all(|byte| *byte == value))
}

/// **Case 11c: a capture sampled through smithay with no CPU wait reads the
/// finished picture**, a hundred times running, under load. The rounded
/// pass's route: `render_texture_from_to`, whose `wait_for_upload` waits on
/// the GPU (smithay `gles/mod.rs:2703`).
fn no_wait_through_smithay(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX0: a capture sampled with no CPU wait, through smithay ===");
    let mut source: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (1024, 1024).into())
        .map_err(|err| anyhow!("{err}"))?;
    let mut sink: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (64, 64).into())
        .map_err(|err| anyhow!("{err}"))?;
    for round in 0..100 {
        let (colour, byte) = shade(round);
        draw_heavy(renderer, &mut source, colour)?;
        {
            let mut framebuffer = renderer.bind(&mut sink).map_err(|err| anyhow!("{err}"))?;
            let mut frame = renderer
                .render(&mut framebuffer, (64, 64).into(), Transform::Normal)
                .map_err(|err| anyhow!("{err}"))?;
            frame
                .render_texture_from_to(
                    &source,
                    Rectangle::from_size((1024.0, 1024.0).into()),
                    Rectangle::from_size((64, 64).into()),
                    &[Rectangle::from_size((64, 64).into())],
                    &[],
                    Transform::Normal,
                    1.0,
                    None,
                    &[],
                )
                .map_err(|err| anyhow!("sampling the capture: {err}"))?;
            frame
                .finish()
                .map_err(|err| anyhow!("{err}"))?
                .wait()
                .map_err(|err| anyhow!("waiting for the sample: {err:?}"))?;
        }
        if !all_are(&read(renderer, &mut sink, 64)?, byte) {
            return Err(anyhow!(
                "round {round}: a capture sampled with no CPU wait read pixels it had not finished"
            ));
        }
    }
    println!("  100 rounds of 500 fills, sampled at once through smithay: every pixel right");
    Ok(())
}

const PLAIN_VERTEX: &str = "attribute vec2 position; varying vec2 uv; void main() { uv = position * 0.5 + 0.5; gl_Position = vec4(position, 0.0, 1.0); }\n";
const PLAIN_FRAGMENT: &str = "precision mediump float; uniform sampler2D tex; varying vec2 uv; void main() { gl_FragColor = texture2D(tex, uv); }\n";

/// Compile and link a two-stage program, and a vertex buffer holding one
/// full-target quad, as `warp.rs` keeps its vertices in a buffer of its own.
///
/// # Safety
/// A context is current (inside `with_context`).
unsafe fn plain_program(gl: &ffi::Gles2) -> Option<(ffi::types::GLuint, ffi::types::GLuint)> {
    unsafe {
        let stage = |kind, source: &str| {
            let shader = gl.CreateShader(kind);
            let length = i32::try_from(source.len()).unwrap_or(0);
            gl.ShaderSource(shader, 1, [source.as_ptr().cast()].as_ptr(), &raw const length);
            gl.CompileShader(shader);
            let mut done = 0;
            gl.GetShaderiv(shader, ffi::COMPILE_STATUS, &raw mut done);
            (done != 0).then_some(shader)
        };
        let (vertex, fragment) = (
            stage(ffi::VERTEX_SHADER, PLAIN_VERTEX)?,
            stage(ffi::FRAGMENT_SHADER, PLAIN_FRAGMENT)?,
        );
        let program = gl.CreateProgram();
        gl.AttachShader(program, vertex);
        gl.AttachShader(program, fragment);
        gl.BindAttribLocation(program, 0, c"position".as_ptr().cast());
        gl.LinkProgram(program);
        let mut linked = 0;
        gl.GetProgramiv(program, ffi::LINK_STATUS, &raw mut linked);
        if linked == 0 {
            return None;
        }
        let quad: [f32; 12] = [
            -1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0,
        ];
        let mut buffer = 0;
        gl.GenBuffers(1, &raw mut buffer);
        gl.BindBuffer(ffi::ARRAY_BUFFER, buffer);
        gl.BufferData(
            ffi::ARRAY_BUFFER,
            std::mem::size_of_val(&quad) as isize,
            quad.as_ptr().cast(),
            ffi::STATIC_DRAW,
        );
        gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
        Some((program, buffer))
    }
}

/// **Case 11d: the same, sampled the way `warp.rs` samples**: the raw texture
/// name, through a program of its own inside `frame.with_context`, with no
/// `glWaitSync` anywhere. Only GL's single-context order makes this right.
fn no_wait_through_raw_gl(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX0: a capture sampled with no CPU wait, through raw GL as the warp does ===");
    let mut source: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (1024, 1024).into())
        .map_err(|err| anyhow!("{err}"))?;
    let mut sink: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, (64, 64).into())
        .map_err(|err| anyhow!("{err}"))?;
    // SAFETY: `with_context` makes the context current for the call.
    let (program, buffer) = renderer
        .with_context(|gl| unsafe { plain_program(gl) })
        .map_err(|err| anyhow!("{err}"))?
        .ok_or_else(|| anyhow!("the plain sampling program did not build"))?;
    for round in 0..100 {
        let (colour, byte) = shade(round);
        draw_heavy(renderer, &mut source, colour)?;
        let texture = source.tex_id();
        {
            let mut framebuffer = renderer.bind(&mut sink).map_err(|err| anyhow!("{err}"))?;
            let mut frame = renderer
                .render(&mut framebuffer, (64, 64).into(), Transform::Normal)
                .map_err(|err| anyhow!("{err}"))?;
            // SAFETY: inside the frame's context; every name is this context's.
            frame
                .with_context(|gl| unsafe {
                    gl.UseProgram(program);
                    gl.ActiveTexture(ffi::TEXTURE0);
                    gl.BindTexture(ffi::TEXTURE_2D, texture);
                    gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
                    gl.Disable(ffi::BLEND);
                    gl.BindBuffer(ffi::ARRAY_BUFFER, buffer);
                    gl.EnableVertexAttribArray(0);
                    gl.VertexAttribDivisor(0, 0);
                    gl.VertexAttribPointer(0, 2, ffi::FLOAT, ffi::FALSE, 0, std::ptr::null());
                    gl.DrawArrays(ffi::TRIANGLES, 0, 6);
                    gl.DisableVertexAttribArray(0);
                    gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
                    gl.BindTexture(ffi::TEXTURE_2D, 0);
                    gl.UseProgram(0);
                })
                .map_err(|err| anyhow!("{err}"))?;
            frame
                .finish()
                .map_err(|err| anyhow!("{err}"))?
                .wait()
                .map_err(|err| anyhow!("{err:?}"))?;
        }
        if !all_are(&read(renderer, &mut sink, 64)?, byte) {
            return Err(anyhow!(
                "round {round}: a capture sampled through raw GL with no CPU wait read pixels it had not finished"
            ));
        }
    }
    println!("  100 rounds of 500 fills, sampled at once through raw GL: every pixel right");
    Ok(())
}

/// **The control: the same check with a reader on another display**, which no
/// GL order covers. A dmabuf is drawn the same way, its fence dropped, and read
/// at once through an independent EGL display (`join_readback`, readback.cpp).
/// Seeing a stale read even once proves 11c and 11d could see a race; never
/// seeing one is recorded, and then 11c and 11d are regression guards only.
pub(crate) fn cross_control(
    renderer: &mut GlesRenderer,
    gbm: &smithay::backend::allocator::gbm::GbmDevice<smithay::backend::drm::DrmDeviceFd>,
    node: &str,
) -> Result<()> {
    println!("\n=== FX0 control: the same draw read from another display, with no wait ===");
    let side = 1024;
    let target = super::target::allocate(gbm, side, side).map_err(|err| anyhow!("{err}"))?;
    let (fd, stride, modifier, fourcc) = target.as_ffi().map_err(|err| anyhow!("{err}"))?;
    let node_c = std::ffi::CString::new(node)?;
    let mut stale = 0;
    for round in 0..100 {
        let (colour, byte) = shade(round);
        let mut buffer = target.dmabuf.clone();
        {
            let whole = Rectangle::from_size((side, side).into());
            let mut framebuffer = renderer.bind(&mut buffer).map_err(|err| anyhow!("{err}"))?;
            let mut frame = renderer
                .render(&mut framebuffer, (side, side).into(), Transform::Normal)
                .map_err(|err| anyhow!("{err}"))?;
            for _ in 0..499 {
                frame
                    .draw_solid(whole, &[whole], Color32F::new(0.5, 0.5, 0.5, 1.0))
                    .map_err(|err| anyhow!("{err}"))?;
            }
            frame
                .draw_solid(whole, &[whole], Color32F::new(colour[0], colour[1], colour[2], 1.0))
                .map_err(|err| anyhow!("{err}"))?;
            drop(frame.finish().map_err(|err| anyhow!("{err}"))?);
        }
        let mut raw = vec![0_u8; usize::try_from(side * side * 4).unwrap_or(0)];
        // SAFETY: the fd and the layout are the buffer's own; `raw` is big enough.
        let rc = unsafe {
            super::join_readback(node_c.as_ptr(), fd, side, side, stride, modifier, fourcc, raw.as_mut_ptr())
        };
        if rc != 0 {
            return Err(anyhow!("the independent readback failed with rc={rc}"));
        }
        if !all_are(&raw, byte) {
            stale += 1;
        }
    }
    println!("  a reader on another display saw an unfinished picture in {stale} of 100 rounds");
    Ok(())
}
