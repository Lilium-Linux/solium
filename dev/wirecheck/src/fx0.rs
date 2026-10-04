//! Phase 0 of the shader work, on a real GPU: the cases `cargo test` cannot
//! reach because they need a GL context. Run alone with `WIRECHECK_ONLY=fx0`,
//! and after case 11 in a full run. Each prints a `=== FX0:` heading.

use anyhow::{Result, anyhow};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind as _, Color32F, Frame as _, Offscreen as _, Renderer as _};
use smithay::utils::{Rectangle, Transform};

#[path = "../../../crates/solium/src/gputime.rs"]
#[allow(dead_code, reason = "the compositor's timer, of which this case needs part")]
mod gputime;

/// Every FX0 case, in order.
pub(crate) fn all(renderer: &mut GlesRenderer) -> Result<()> {
    gpu_timestamps(renderer)?;
    closed_outside_a_frame(renderer)?;
    Ok(())
}

/// **Case 11b, second half: a region closed outside a frame resolves at the
/// first idle after its work is done.** The TTY's shape: `open` before a
/// frame and `close` after its `finish`, as `tty.rs` stamps an output around
/// `render_frame`; the frame's fence not waited on (the kernel waits on it);
/// 10 ms for the flip; then one `idle`, as the vblank handler calls it. Ten
/// passes, and every one must be in at that first idle, or the report
/// waiting for it goes as `late` (`Timer::close`).
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
        {
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
            // Not waited on: on the TTY the kernel waits on this fence.
            let _sync = frame.finish().map_err(|err| anyhow!("finishing: {err}"))?;
        }
        timer.close(renderer, stamp);
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
