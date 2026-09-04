//! Writing a rendered frame to a file.
//!
//! A compositor cannot be tested by looking at it: the developer's screenshot
//! tool captures the *host* session, which proves nothing about what Solium
//! composited, and using a shell's own capture to test that shell is circular.
//! So Solium reads its own framebuffer back.
//!
//! The format is binary PPM — no encoder dependency, and every image tool reads
//! it. Enabled per run:
//!
//! ```sh
//! SOLIUM_CAPTURE=/tmp/frame.ppm ./solium
//! ```

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use smithay::{
    backend::{allocator::Fourcc, renderer::ExportMem},
    utils::Rectangle,
};

/// Where to write the next captured frame, if a capture was asked for.
pub(crate) fn requested() -> Option<PathBuf> {
    std::env::var_os("SOLIUM_CAPTURE").map(PathBuf::from)
}

/// When to capture, if a moment was named instead of "once a window settles".
///
/// Naming a moment is what makes a capture of an *animation* possible: the
/// interesting frames are the ones part-way through.
pub(crate) fn capture_at() -> Option<std::time::Duration> {
    millis_from_env("SOLIUM_CAPTURE_AT")
}

/// When to toggle overview, for exercising the transform without a keyboard.
pub(crate) fn overview_at() -> Option<std::time::Duration> {
    millis_from_env("SOLIUM_OVERVIEW_AT")
}

fn millis_from_env(name: &str) -> Option<std::time::Duration> {
    let raw = std::env::var(name).ok()?;
    match raw.trim().parse::<u64>() {
        Ok(millis) => Some(std::time::Duration::from_millis(millis)),
        Err(err) => {
            tracing::warn!(?err, variable = name, value = raw, "not a number, ignoring");
            None
        }
    }
}

/// Read a rendered frame back off the GPU and write it out.
///
/// Reading the framebuffer back is documented to invalidate the current bind,
/// and does: the caller must not present the frame it captured.
pub(crate) fn take_frame<R: ExportMem>(
    renderer: &mut R,
    framebuffer: &R::Framebuffer<'_>,
    width: i32,
    height: i32,
    path: &Path,
) -> Result<()>
where
    R::Error: Send + Sync + 'static,
{
    // Physical and buffer coordinates coincide at scale 1, but they are
    // different types, so the conversion is explicit.
    let region = Rectangle::from_size((width, height).into());
    let mapping = renderer
        .copy_framebuffer(framebuffer, region, Fourcc::Argb8888)
        .context("copying the framebuffer")?;
    let pixels = renderer
        .map_texture(&mapping)
        .context("mapping the copied framebuffer")?;
    write_bgra(path, width, height, pixels)
}

/// Write BGRA pixel data as a binary PPM.
///
/// Two conversions happen here, both because of how the data arrives:
///
/// * **Channel order.** The renderer hands back the framebuffer's native byte
///   order, which is BGRA for the format we ask for; PPM is RGB. Swapped here
///   rather than by asking the GPU for a conversion it may not support.
/// * **Row order.** A GL framebuffer's origin is bottom-left and PPM's is
///   top-left, so rows are emitted in reverse. Without this the capture is
///   vertically mirrored — legible enough to look plausible, subtle enough to
///   be mistaken for a compositor bug.
fn write_bgra(path: &Path, width: i32, height: i32, bgra: &[u8]) -> Result<()> {
    let (width, height) = (width.max(0) as usize, height.max(0) as usize);
    let expected = width * height * 4;
    anyhow::ensure!(
        bgra.len() >= expected,
        "frame is {} bytes, expected at least {expected} for {width}x{height}",
        bgra.len()
    );

    let mut out = Vec::with_capacity(expected / 4 * 3 + 32);
    out.extend_from_slice(format!("P6\n{width} {height}\n255\n").as_bytes());
    for row in bgra[..expected].chunks_exact(width * 4).rev() {
        let (pixels, _) = row.as_chunks::<4>();
        for [blue, green, red, _alpha] in pixels {
            out.extend_from_slice(&[*red, *green, *blue]);
        }
    }

    let mut file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    file.write_all(&out)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
