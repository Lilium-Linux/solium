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
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    input::pointer::CursorImageStatus,
    utils::{Logical, Point, Rectangle, Transform},
};

use crate::qml;

/// How big the cursor image is, in logical pixels.
const SIZE: i32 = 24;

/// Where the point of the arrow is within that image.
///
/// The arrow is drawn with its tip in the top-left corner, so the buffer is
/// placed at the pointer position directly. Kept named rather than assumed,
/// because a cursor drawn a few pixels off its hotspot is maddening to use and
/// almost impossible to see in a screenshot.
const HOTSPOT: (i32, i32) = (0, 0);

/// Our own pointer, rasterised once and reused.
#[derive(Debug)]
pub(crate) struct Cursor {
    scene: qml::Scene,
    buffer: Option<MemoryRenderBuffer>,
}

impl Cursor {
    pub(crate) fn new() -> Result<Self> {
        qml::start()?;
        let scene = qml::Scene::new(&qml_path(), SIZE, SIZE)?;
        Ok(Self {
            scene,
            buffer: None,
        })
    }

    /// The cursor as something to draw, at `location`.
    ///
    /// `Kind::Cursor` is not decoration: it is what lets the DRM backend put
    /// this on the hardware cursor plane, which moves the pointer without
    /// redrawing the screen behind it.
    pub(crate) fn element<R>(
        &mut self,
        renderer: &mut R,
        location: Point<f64, Logical>,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let rendered = match self.scene.render() {
            Ok(rendered) => rendered,
            Err(err) => {
                tracing::warn!(?err, "the cursor did not render");
                return None;
            }
        };

        let size = (SIZE, SIZE);
        let fresh = self.buffer.is_none();
        if fresh {
            self.buffer = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
        }
        let buffer = self.buffer.as_mut()?;

        if rendered.changed || fresh {
            let mut context = buffer.render();
            let copy = context.draw(|target| {
                let row_bytes = usize::try_from(SIZE.max(0)).unwrap_or_default() * 4;
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

        let position = (
            location.x - f64::from(HOTSPOT.0),
            location.y - f64::from(HOTSPOT.1),
        );

        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            buffer,
            None,
            None,
            None,
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
