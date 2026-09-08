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
    // `implicit = false`: keep the modifier GBM actually reports.
    //
    // This used to be `true`, on the strength of smithay's own doc comment --
    // "gbm might otherwise give us the underlying or a non-sensical modifier".
    // Measured wrong on this driver: `gbm_bo_get_modifier` on a plain
    // `create_buffer_object` (no modifier negotiated) returns a real NVIDIA
    // block-linear modifier, and forcing it to `Modifier::Invalid` is what
    // made the C++ side omit the EGL modifier attributes on import, which
    // this driver then rejects outright --
    // `glEGLImageTargetTexture2DOES failed 0x502` (GL_INVALID_OPERATION) --
    // rather than silently falling back to linear. `Invalid` was never a safe
    // default here; it was a different bug that also imported clean.
    //
    // This is not modifier *negotiation* -- there is no candidate-list query
    // against the render node here, only the modifier this one legacy call
    // happened to pick. It works because that happens to be what this driver
    // wants when asked for a renderable buffer with no constraints. A future
    // driver where that is not true would need `create_buffer_object_with_modifiers`
    // fed by an actual queried list, which needs an EGL display this module
    // does not have.
    let buffer = GbmBuffer::from_bo(bo, false);
    let dmabuf = buffer.export().context("exporting the scene buffer")?;
    Ok(Target {
        dmabuf,
        width,
        height,
    })
}

impl Target {
    /// What the QML host needs to import this: fd, stride, modifier, fourcc.
    ///
    /// The returned fd is borrowed from `self.dmabuf` for the duration of the
    /// `solium_qml_scene_new_gpu` call it is handed to: EGL dup's what it needs
    /// at import (`eglCreateImageKHR` takes its own reference), so the caller
    /// must not let it outlive that one call, and must not close it itself --
    /// closing it would close `self.dmabuf`'s own fd out from under `Target`.
    pub(crate) fn as_ffi(&self) -> Result<(RawFd, i32, u64, u32)> {
        // A real (non-`Invalid`) modifier can describe a multi-plane layout,
        // which taking plane 0 alone and ignoring the rest would hand to Qt as
        // if it were the whole image -- wrong pixels, not a crash, and nothing
        // in the return value would say why. Argb8888 from `allocate` is
        // single-plane on every driver this has been measured against, so this
        // is a should-never-happen guard, not a format we expect to hit.
        let planes = self.dmabuf.num_planes();
        if planes != 1 {
            return Err(anyhow!(
                "the scene buffer has {planes} planes, not the 1 as_ffi assumes"
            ));
        }
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

    /// 20000 and 2560 both sit far from `MAX_SIDE`, so an off-by-one in the
    /// comparison would pass unnoticed. Only a test at the boundary itself
    /// checks the boundary.
    #[test]
    fn the_cap_is_inclusive_of_max_side_and_no_further() {
        assert!(size_is_sane(super::MAX_SIDE, super::MAX_SIDE));
        assert!(!size_is_sane(super::MAX_SIDE + 1, super::MAX_SIDE));
        assert!(!size_is_sane(super::MAX_SIDE, super::MAX_SIDE + 1));
    }
}
