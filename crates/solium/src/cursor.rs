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
    qml::{
        self,
        paint::{Gpu, Placement},
    },
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

/// How a rasterised pointer reaches the screen.
///
/// Not a preference, and not this module's decision: Qt fixes its scene graph
/// for the life of the process and a host that came up on one backend refuses
/// scenes of the other kind, so this follows `qml::on_gpu` — see
/// [`qml::Scene::for_host`], which is where it is actually decided.
#[derive(Debug)]
enum Backing {
    /// Rasterised on the CPU and uploaded. The scale is the one the buffer
    /// currently holds, so moving the pointer between monitors at different
    /// scales rasterises again rather than stretching.
    Memory {
        buffer: Option<MemoryRenderBuffer>,
        scale: f64,
    },
    /// Drawn by Qt into a dmabuf we allocated, which the compositor samples.
    /// A scale change is a new buffer, which [`Gpu`] handles by rebinding.
    Gpu(Gpu),
}

/// Our own pointer, rasterised once and reused.
#[derive(Debug)]
pub(crate) struct Cursor {
    scene: qml::Scene,
    backing: Backing,
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
                // allocated right the first time and only a scale change moves
                // it.
                Backing::Gpu(Gpu::new((SIZE, SIZE)))
            } else {
                Backing::Memory {
                    buffer: None,
                    scale: 1.0,
                }
            },
        })
    }

    /// The cursor as something to draw, at `location`.
    ///
    /// `Kind::Cursor` is not decoration: it is what lets the DRM backend put
    /// this on the hardware cursor plane, which moves the pointer without
    /// redrawing the screen behind it.
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

        // Physical, and the hotspot is logical, so both go through the scale.
        let position = (
            (location.x - f64::from(HOTSPOT.0)) * scale,
            (location.y - f64::from(HOTSPOT.1)) * scale,
        );

        // Two fields of one struct, borrowed at once.
        let Self { scene, backing } = self;
        match backing {
            Backing::Gpu(gpu) => gpu
                .element(
                    scene,
                    renderer,
                    (edge, edge),
                    scale,
                    Placement {
                        position,
                        // Mapped down to 24 logical pixels, which the output
                        // scale takes back up to `edge`.
                        size: (SIZE, SIZE).into(),
                        alpha: 1.0,
                        kind: Kind::Cursor,
                    },
                )
                .map(Element::Screen),
            Backing::Memory { .. } => self
                .in_memory(renderer, position, edge, scale)
                .map(Element::Chrome),
        }
    }

    /// The software path, unchanged: Qt rasterises into a `QImage` and the
    /// compositor uploads it.
    fn in_memory(
        &mut self,
        renderer: &mut GlesRenderer,
        position: (f64, f64),
        edge: i32,
        scale: f64,
    ) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
        let Backing::Memory { scale: held, .. } = self.backing else {
            return None;
        };
        let rescaled = (held - scale).abs() > f64::EPSILON;
        if rescaled {
            self.scene.resize(edge, edge, scale);
        }

        let rendered = match self.scene.render() {
            Ok(rendered) => rendered,
            Err(err) => {
                tracing::warn!(?err, "the cursor did not render");
                return None;
            }
        };

        let size = (edge, edge);
        let Backing::Memory {
            buffer: slot,
            scale: held,
        } = &mut self.backing
        else {
            return None;
        };
        *held = scale;
        let fresh = slot.is_none() || rescaled;
        if fresh {
            *slot = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
        }
        let buffer = slot.as_mut()?;

        if rendered.changed || fresh {
            let mut context = buffer.render();
            let copy = context.draw(|target| {
                let row_bytes = usize::try_from(edge.max(0)).unwrap_or_default() * 4;
                for (row, destination) in target.chunks_exact_mut(row_bytes).enumerate() {
                    let start = row * rendered.stride;
                    let Some(source) = rendered.pixels.get(start..start + row_bytes) else {
                        return Err(());
                    };
                    destination.copy_from_slice(source);
                }
                Ok(vec![Rectangle::from_size(size.into())])
            });
            if copy.is_err() {
                tracing::warn!("the cursor image was smaller than its buffer");
                return None;
            }
        }

        // The whole buffer in its own pixels, mapped down to 24 logical
        // pixels, which the output scale takes back up to `edge`.
        let source = Rectangle::from_size((f64::from(edge), f64::from(edge)).into());
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
    }
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
