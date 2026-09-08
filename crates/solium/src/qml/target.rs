//! The buffer Qt renders a scene into.
//!
//! Allocated by us through GBM and handed to Qt as a texture, because Qt
//! cannot be made to use our EGL context — see
//! `docs/spikes/2026-09-04-qml-in-compositor.md`. Sharing the buffer is the
//! route that remains, and it is how two processes would do it anyway.

use anyhow::{Context, Result, anyhow};
use smithay::backend::allocator::{
    Buffer as _, Fourcc, Modifier,
    dmabuf::{AsDmabuf, Dmabuf},
    gbm::{GbmBuffer, GbmBufferFlags, GbmDevice},
};
use smithay::backend::drm::DrmDeviceFd;
use std::os::fd::{AsRawFd, RawFd};

/// The largest scene we will allocate, per side.
///
/// A style's `bleed` is author-controlled, so this is the difference between
/// a typo and 1.6 GB of video memory.
const MAX_SIDE: i32 = 8192;

fn size_is_sane(width: i32, height: i32) -> bool {
    width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE
}

pub(crate) struct Target {
    pub(crate) dmabuf: Dmabuf,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

/// Allocate a buffer both sides can use.
pub(crate) fn allocate(gbm: &GbmDevice<DrmDeviceFd>, width: i32, height: i32) -> Result<Target> {
    if !size_is_sane(width, height) {
        return Err(anyhow!("a scene of {width}x{height} is not a size"));
    }
    // `GbmDevice` here is `gbm::Device` itself (smithay re-exports it under
    // that name), so `create_buffer_object` hands back the `gbm` crate's own
    // `BufferObject`, not smithay's `GbmBuffer`. `AsDmabuf::export` is only
    // implemented for the latter, so the object is wrapped before exporting.
    let bo = gbm
        .create_buffer_object::<()>(
            u32::try_from(width)?,
            u32::try_from(height)?,
            Fourcc::Argb8888,
            GbmBufferFlags::RENDERING,
        )
        .context("allocating a scene buffer")?;
    // `implicit = true`: this call (unlike `create_buffer_object_with_modifiers`)
    // predates modifier negotiation, so GBM can hand back a modifier that
    // doesn't mean anything. Telling smithay it's implicit makes it report
    // `Modifier::Invalid` instead of that nonsense value — the same thing
    // `GbmAllocator` does internally when it falls back to this call.
    let buffer = GbmBuffer::from_bo(bo, true);
    let dmabuf = buffer.export().context("exporting the scene buffer")?;
    Ok(Target {
        dmabuf,
        width,
        height,
    })
}

impl Target {
    /// What the QML host needs to import this: fd, stride, modifier, fourcc.
    pub(crate) fn as_ffi(&self) -> Result<(RawFd, i32, u64, u32)> {
        let fd = self
            .dmabuf
            .handles()
            .next()
            .ok_or_else(|| anyhow!("the scene buffer has no plane"))?
            .as_raw_fd();
        let stride = self
            .dmabuf
            .strides()
            .next()
            .ok_or_else(|| anyhow!("the scene buffer has no stride"))?;
        let modifier: Modifier = self.dmabuf.format().modifier;
        Ok((
            fd,
            i32::try_from(stride)?,
            u64::from(modifier),
            self.dmabuf.format().code as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::size_is_sane;

    /// A zero or negative size reaches GBM as a huge unsigned number and
    /// allocates something enormous, or fails in a way that names neither the
    /// scene nor the size.
    #[test]
    fn a_zero_sized_target_is_refused() {
        assert!(!size_is_sane(0, 100));
        assert!(!size_is_sane(100, 0));
        assert!(!size_is_sane(-4, 100));
    }

    /// The cap exists because `bleed` is author-controlled: a style asking for
    /// 20000 pixels of bleed must be told no, not allocate 1.6GB.
    #[test]
    fn an_absurd_size_is_refused() {
        assert!(!size_is_sane(20_000, 20_000));
        assert!(size_is_sane(2560, 1440));
    }
}
