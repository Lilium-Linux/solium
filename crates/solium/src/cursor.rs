//! The pointer.
//!
//! Two cases, and both have to work. A client that sets its own cursor gets it
//! drawn — an I-beam over text, a resize arrow on an edge — and everything else
//! gets ours, drawn from QML through the same design system as the window
//! frames, so the pointer belongs to the same look as the rest.
//!
//! Nested, none of this existed: the host compositor drew the cursor over our
//! window and we never had to think about it. On the hardware nothing else
//! will, and an invisible pointer is not a cosmetic problem — it is
//! indistinguishable from input being dead, which is exactly how it was
//! reported the first time this ran on a real screen.

use std::path::PathBuf;

use anyhow::Result;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind as _, ExportMem as _,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    input::pointer::CursorImageStatus,
    utils::{Logical, Point, Rectangle, Transform},
};

use crate::{
    qml::{self, paint::Gpu},
    render::Element,
};

/// How big the cursor image is, in logical pixels.
const SIZE: i32 = 24;

/// Where the point of the arrow is within that image.
///
/// The arrow is drawn with its tip in the top-left corner, so the buffer is
/// placed at the pointer position directly. Kept named rather than assumed,
/// because a cursor drawn a few pixels off its hotspot is maddening to use and
/// almost impossible to see in a screenshot.
const HOTSPOT: (i32, i32) = (0, 0);

/// How many device sizes of the pointer are kept at once.
///
/// One per monitor scale in use, and two is the realistic number. The cap is
/// here because the key is a pixel size rather than a monitor: a script that
/// animates an output's scale would otherwise mint a buffer per step and keep
/// every one of them.
const KEPT: usize = 4;

/// How a rasterised pointer reaches the screen.
///
/// Not a preference, and not this module's decision: Qt fixes its scene graph
/// for the life of the process and a host that came up on one backend refuses
/// scenes of the other kind, so this follows `qml::on_gpu` — see
/// [`qml::Scene::for_host`], which is where it is actually decided.
///
/// **Both arms end in a `MemoryRenderBuffer`, and that is the point.** The
/// pointer is the one element in the compositor that can occupy a dedicated
/// hardware plane, and smithay reaches a cursor plane only through
/// `RenderElement::underlying_storage` — which has exactly two variants,
/// `Wayland` and `Memory` (`renderer/element/mod.rs:103-109`). A
/// `TextureRenderElement` implements none, inherits the default `None`, and
/// `copy_element_to_cursor_bo` gives up on its first line
/// (`drm/compositor/mod.rs:4190-4193`); so does the pixman fallback
/// (`mod.rs:3269`). There is no `UnderlyingStorage` a dmabuf-backed texture can
/// satisfy, so a GPU cursor drawn as a texture turns every pointer motion on a
/// TTY into a full composite and page flip of the whole output, logged nowhere
/// above `trace!`.
///
/// So on the GPU path Qt still draws the pointer — it has to, a GPU host
/// refuses software scenes — and the result is read straight back out of the
/// dmabuf into a `MemoryRenderBuffer`. That is a readback the other scenes do
/// not pay, and it is affordable here for the reason the whole trade is
/// lopsided: the pointer is 24 logical pixels, so there is almost no
/// rasterisation to save by putting it on the GPU, and it changes on a scale or
/// a theme change rather than per frame.
///
/// Measured rather than assumed — `dev/wirecheck` runs the round trip and
/// prints what it costs. On this machine the whole import, bind,
/// `copy_framebuffer` and map is **~60 µs**, and near enough the same at 24x24
/// (63.5 µs) as at 48x48 (59.8 µs), so it is the round trip and not the pixels.
/// It is paid **once per size per change**, not per frame — see the cache
/// below, which is what makes that true.
#[derive(Debug)]
enum Backing {
    /// Qt rasterises into a `QImage` and we copy out of it.
    Memory,
    /// Qt draws into a dmabuf we allocated; we sample it and read it back.
    Gpu(Gpu),
}

/// Our own pointer, rasterised once per size and reused.
#[derive(Debug)]
pub(crate) struct Cursor {
    scene: qml::Scene,
    backing: Backing,
    /// One uploadable buffer per device size the pointer has been asked for.
    buffers: Kept<MemoryRenderBuffer>,
}

/// What the pointer has already been drawn at, newest last.
///
/// Keyed by device size and not by "the current scale", which is what this was.
/// `render.rs` builds the pointer for *every* output, deliberately and with a
/// comment saying so, so a desktop with a 1x and a 2x monitor asks for 24 and
/// then 48 on every single frame, for ever. With one buffer that meant
/// re-rasterising twice a frame on the software path — and on the GPU path a GBM
/// allocation, a dmabuf handed to Qt, an EGLImage, a full re-render of
/// `cursor.qml` through the curve renderer and an import, twice a frame, with
/// full damage reported on both outputs each time. None of it needed any user
/// action beyond owning two monitors.
///
/// Generic over what is kept only so the policy can be tested without Qt: what
/// costs anything here is how often this misses, and that is arithmetic rather
/// than graphics.
#[derive(Debug)]
struct Kept<T> {
    held: Vec<(i32, T)>,
}

impl<T> Default for Kept<T> {
    fn default() -> Self {
        Self { held: Vec::new() }
    }
}

impl<T> Kept<T> {
    fn has(&self, edge: i32) -> bool {
        self.held.iter().any(|(size, _)| *size == edge)
    }

    fn get(&mut self, edge: i32) -> Option<&mut T> {
        self.held
            .iter_mut()
            .find(|(size, _)| *size == edge)
            .map(|(_, held)| held)
    }

    /// Everything kept is the previous picture. See `Cursor::element`.
    fn clear(&mut self) {
        self.held.clear();
    }

    /// Oldest out first. [`KEPT`] is four and a desktop uses two, so this is a
    /// guard against a pathological scale rather than an eviction policy
    /// anything is expected to reach.
    fn push(&mut self, edge: i32, value: T) {
        self.held.retain(|(size, _)| *size != edge);
        if self.held.len() >= KEPT {
            self.held.remove(0);
        }
        self.held.push((edge, value));
    }
}

impl Cursor {
    pub(crate) fn new() -> Result<Self> {
        qml::start()?;
        // `SIZE` square to begin with, which at 1x is also the size it stays.
        // It is *not* a scene that is never resized, whatever its buffer being
        // one picture might suggest: 24 is 24 *logical* pixels, so the pointer
        // crossing onto a 2x monitor needs a 48-pixel one and the GPU path
        // rebinds onto a new buffer to get it. See `Cursor::element`.
        let scene = qml::Scene::for_host(&qml_path(), SIZE, SIZE, None)?;
        Ok(Self {
            scene,
            backing: if qml::on_gpu() {
                // The size the scene really is, unlike the shell surfaces:
                // there is nothing to discover about a pointer's size, so it is
                // allocated right the first time and only a new scale moves it.
                Backing::Gpu(Gpu::new((SIZE, SIZE)))
            } else {
                Backing::Memory
            },
            buffers: Kept::default(),
        })
    }

    /// The cursor as something to draw, at `location`.
    ///
    /// `Kind::Cursor` is not decoration: it is what lets the DRM backend put
    /// this on the hardware cursor plane, which moves the pointer without
    /// redrawing the screen behind it. See [`Backing`] for why that survives on
    /// the GPU path only because this still ends in a `MemoryRenderBuffer`.
    ///
    /// Concrete on `GlesRenderer` rather than generic since the GPU path
    /// arrived, for the reason `ShellSurface::element` gives: taking the
    /// thread's EGL context back off Qt is `EGLContext::make_current`, and
    /// nothing on the `Renderer` traits says where the context is.
    pub(crate) fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        location: Point<f64, Logical>,
        scale: f64,
    ) -> Option<Element> {
        // 24 logical pixels, whatever the monitor is. On a 2x display that is
        // a 48-pixel image, and drawing the 24-pixel one there would leave a
        // pointer a quarter of the size it should be -- which on a HiDPI panel
        // is a pointer you cannot find, and this module exists because an
        // invisible pointer reads as input being dead.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a cursor is 24 logical pixels"
        )]
        let edge = ((f64::from(SIZE) * scale).round() as i32).max(1);

        // Anything kept is the *previous* picture, and the render below is
        // about to spend the flag that says so. Cleared before it rather than
        // after, so the size this frame does not want is re-made when it is
        // next asked for instead of being served stale for ever.
        //
        // In practice this fires once, on the first frame: `cursor.qml` has no
        // animation and nothing writes its properties, so after the pointer has
        // been drawn once it never asks to be drawn again.
        if self.scene.needs_render() {
            self.buffers.clear();
        }

        // The size actually in hand, which is `edge` unless a GPU rebind failed
        // and the scene is frozen on a smaller buffer.
        let held = if self.buffers.has(edge) {
            edge
        } else {
            self.fill(renderer, edge, scale)?
        };
        let buffer = self.buffers.get(held)?;

        // Physical, and the hotspot is logical, so both go through the scale.
        let position = (
            (location.x - f64::from(HOTSPOT.0)) * scale,
            (location.y - f64::from(HOTSPOT.1)) * scale,
        );

        // The whole buffer in its own pixels, mapped down to 24 logical
        // pixels, which the output scale takes back up to `held`. When `held`
        // is `edge` — every case but a frozen scene — those two are the same
        // number, which is also what `copy_element_to_cursor_bo` requires
        // before it will use the plane's fast path: it refuses any element
        // whose src and drawn size disagree.
        let source = Rectangle::from_size((f64::from(held), f64::from(held)).into());
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            buffer,
            None,
            Some(source),
            Some((SIZE, SIZE).into()),
            Kind::Cursor,
        )
        .inspect_err(|err| tracing::warn!(?err, "could not upload the cursor"))
        .ok()
        .map(Element::Chrome)
    }

    /// Draw the pointer at `edge` pixels and keep the result. Returns the size
    /// that was actually produced.
    fn fill(&mut self, renderer: &mut GlesRenderer, edge: i32, scale: f64) -> Option<i32> {
        // Two fields of one struct, borrowed at once.
        let Self {
            scene,
            backing,
            buffers,
        } = self;
        let (pixels, stride, held) = match backing {
            Backing::Memory => {
                scene.resize(edge, edge, scale);
                match scene.render() {
                    Ok(rendered) => (Pixels::Borrowed(rendered.pixels), rendered.stride, edge),
                    Err(err) => {
                        tracing::warn!(?err, "the cursor did not render");
                        return None;
                    }
                }
            }
            Backing::Gpu(gpu) => {
                let (texture, size) = {
                    let shown = gpu.sample(scene, renderer, (edge, edge), scale)?;
                    (shown.texture.clone(), shown.size)
                };
                let read = read_back(renderer, texture, size)?;
                let stride = usize::try_from(size.0.max(0)).unwrap_or_default() * 4;
                (Pixels::Owned(read), stride, size.0)
            }
        };

        let row_bytes = usize::try_from(held.max(0)).unwrap_or_default() * 4;
        let mut buffer =
            MemoryRenderBuffer::new(Fourcc::Argb8888, (held, held), 1, Transform::Normal, None);
        let mut context = buffer.render();
        let copy = context.draw(|target| {
            for (row, destination) in target.chunks_exact_mut(row_bytes).enumerate() {
                let start = row * stride;
                let Some(source) = pixels.as_ref().get(start..start + row_bytes) else {
                    return Err(());
                };
                destination.copy_from_slice(source);
            }
            Ok(vec![Rectangle::from_size((held, held).into())])
        });
        if copy.is_err() {
            tracing::warn!("the cursor image was smaller than its buffer");
            return None;
        }
        drop(context);

        buffers.push(held, buffer);
        Some(held)
    }
}

/// Whichever of the two paths produced the pixels, as one slice.
///
/// The software path borrows them from the scene and the GPU path owns them, so
/// the copy below cannot name a single type without this.
enum Pixels<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl AsRef<[u8]> for Pixels<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Borrowed(pixels) => pixels,
            Self::Owned(pixels) => pixels,
        }
    }
}

/// Read a scene's dmabuf back into ordinary memory.
///
/// The pointer's whole reason for being on this path — see [`Backing`]. Top-down
/// premultiplied ARGB8888, tightly packed, which is exactly what
/// `MemoryRenderBuffer` wants and what `import_memory` would have been handed on
/// the software path; `dev/wirecheck` asserts that byte for byte rather than
/// leaving it to the orientation conventions of three different APIs.
///
/// Binds a framebuffer, so it must not run underneath another bind — every
/// backend builds its elements before it binds anything, which is the same
/// convention `offscreen::capture` relies on.
fn read_back(
    renderer: &mut GlesRenderer,
    texture: smithay::backend::renderer::gles::GlesTexture,
    size: (i32, i32),
) -> Option<Vec<u8>> {
    let mut texture = texture;
    let framebuffer = renderer
        .bind(&mut texture)
        .inspect_err(|err| tracing::warn!(?err, "could not bind the cursor's buffer to read it"))
        .ok()?;
    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size(size.into()),
            Fourcc::Argb8888,
        )
        .inspect_err(|err| tracing::warn!(?err, "could not copy the cursor's buffer"))
        .ok();
    drop(framebuffer);
    // Whatever happened, leave nothing of ours bound: the next thing to bind is
    // an output's own buffer, and a framebuffer left over here would take the
    // whole frame with it.
    crate::warp::release_framebuffer(renderer);
    let pixels = renderer
        .map_texture(&mapping?)
        .inspect_err(|err| tracing::warn!(?err, "could not map the cursor's buffer"))
        .ok()?
        .to_vec();
    Some(pixels)
}

fn qml_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SOLIUM_QML_CURSOR") {
        return PathBuf::from(path);
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/cursor.qml"))
}

/// The pointer as the compositor holds it: what to show, and what to draw it
/// with.
///
/// One place, because the two are only meaningful together — a status with
/// nothing able to render it is an invisible pointer, which is the bug this
/// module exists to have fixed.
#[derive(Debug)]
pub(crate) struct Pointer {
    /// What the pointer should look like, as clients and Smithay set it.
    pub(crate) status: CursorImageStatus,
    art: Option<Cursor>,
    /// Set once QML has failed, so a broken scene costs one error and not one
    /// per frame for the life of the session.
    unavailable: bool,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            status: CursorImageStatus::default_named(),
            art: None,
            unavailable: false,
        }
    }
}

impl Pointer {
    /// Our own arrow, built the first time it is needed.
    ///
    /// Built lazily because a compositor that cannot start QML should still
    /// run — badly, with no pointer, but still be escapable — rather than fail
    /// to launch on a machine where the display is the only way to see why.
    pub(crate) fn art(&mut self) -> Option<&mut Cursor> {
        if self.art.is_none() && !self.unavailable {
            match Cursor::new() {
                Ok(cursor) => self.art = Some(cursor),
                Err(err) => {
                    tracing::error!(?err, "no cursor: the pointer will be invisible");
                    self.unavailable = true;
                }
            }
        }
        self.art.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::{KEPT, Kept};

    /// The pointer is built for **every** output on every frame -- `render.rs`
    /// says so in as many words and argues for it -- so a desktop with a 1x and
    /// a 2x monitor asks for 24 and then 48, alternating, for as long as the
    /// session lasts. Before this cache each swing re-rasterised the scene; on
    /// the GPU path it was a GBM allocation, an EGLImage, a full Qt re-render
    /// and an import, twice a frame, for ever.
    ///
    /// The claim is that it costs two misses in total rather than two a frame,
    /// and that is the thing worth a test: it is arithmetic, and it does not
    /// need Qt.
    #[test]
    fn two_monitors_at_two_scales_cost_two_fills_and_then_nothing() {
        let mut kept: Kept<u32> = Kept::default();
        let mut fills = 0;
        for frame in 0..100 {
            for edge in [24, 48] {
                if !kept.has(edge) {
                    fills += 1;
                    kept.push(edge, frame);
                }
            }
        }
        assert_eq!(fills, 2, "the pointer was re-drawn after the first frame");
        assert!(kept.get(24).is_some() && kept.get(48).is_some());
    }

    /// And a theme change has to be able to invalidate every size at once, or
    /// the monitor that is not being looked at keeps the old pointer for ever.
    #[test]
    fn clearing_invalidates_every_size() {
        let mut kept: Kept<u32> = Kept::default();
        kept.push(24, 1);
        kept.push(48, 1);
        kept.clear();
        assert!(!kept.has(24) && !kept.has(48));
    }

    /// The cap is a guard against a script animating an output's scale, which
    /// would otherwise mint a buffer per step and keep every one.
    #[test]
    fn the_cap_evicts_the_oldest() {
        let mut kept: Kept<u32> = Kept::default();
        for edge in 1..=(KEPT as i32 + 2) {
            kept.push(edge, 0);
        }
        assert_eq!(kept.held.len(), KEPT);
        assert!(!kept.has(1), "the oldest size was kept");
        assert!(kept.has(KEPT as i32 + 2), "the newest size was dropped");
    }

    /// Asking for a size already held must not grow the list, or a pointer that
    /// never changes size still evicts everything else eventually.
    #[test]
    fn re_pushing_a_size_replaces_it() {
        let mut kept: Kept<u32> = Kept::default();
        kept.push(24, 1);
        kept.push(24, 2);
        assert_eq!(kept.held.len(), 1);
        assert_eq!(kept.get(24).copied(), Some(2));
    }
}
