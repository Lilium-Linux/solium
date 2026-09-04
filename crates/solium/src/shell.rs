//! The compositor's own surfaces. Today that is the top bar.
//!
//! Authored in QML (`qml/topbar.qml`) and rendered by Qt's scene graph into a
//! texture this process owns — see `qml.rs`. The bar **reserves** its height:
//! `Solium::work_area` excludes it, so windows are placed below it and never
//! underneath it. A bar that windows slide under is a panel; a bar that owns
//! its strip of screen is part of the desktop.
//!
//! QML is rasterised by Qt's software scene graph and uploaded as a memory
//! buffer — see `qml/host.cpp` for why that rather than rendering straight into
//! one of our GL textures. It costs an upload per *changed* frame, which for a
//! strip of chrome is a few hundred kilobytes, and it keeps this module on
//! Smithay's renderer traits rather than reaching into GLES.

use std::{path::PathBuf, time::Duration};

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
    utils::{Rectangle, Transform},
};

use crate::{qml, render::Element};

/// How tall the bar is, and therefore how much screen it takes away from
/// windows.
pub(crate) const BAR_HEIGHT: i32 = 34;

/// What the bar shows. Read from compositor state once per frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct BarState {
    pub(crate) title: String,
    pub(crate) windows: usize,
    pub(crate) overview: bool,
}

/// The top bar: a QML scene and the buffer it is uploaded through.
#[derive(Debug)]
pub(crate) struct Bar {
    scene: qml::Scene,
    buffer: Option<MemoryRenderBuffer>,
    /// Tracked here because `MemoryRenderBuffer` does not report its own size.
    buffer_size: (i32, i32),
    shown: BarState,
}

impl Bar {
    /// Load the bar.
    pub(crate) fn new(width: i32) -> Result<Self> {
        qml::start()?;
        let scene = qml::Scene::new(&qml_path(), width.max(1), BAR_HEIGHT)?;
        tracing::info!(width, height = BAR_HEIGHT, "top bar loaded from QML");
        Ok(Self {
            scene,
            buffer: None,
            buffer_size: (0, 0),
            shown: BarState::default(),
        })
    }

    /// Draw the bar for this frame and return it as a render element.
    ///
    /// `now` is the compositor's clock, and it drives the QML animations —
    /// there is one clock here, and a bar animating off Qt's own timer would
    /// drift against every window transform beside it.
    pub(crate) fn frame<R>(
        &mut self,
        renderer: &mut R,
        width: i32,
        now: Duration,
        state: &BarState,
    ) -> Option<Element<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let width = width.max(1);
        self.scene.resize(width, BAR_HEIGHT);

        // Properties are only pushed when they change: every setProperty is a
        // QML binding re-evaluation, and the title one restarts an animation.
        if *state != self.shown {
            self.scene.set_string("windowTitle", &state.title);
            #[expect(
                clippy::cast_precision_loss,
                reason = "a window count large enough to lose precision is not reachable"
            )]
            self.scene.set_real("windowCount", state.windows as f64);
            self.scene.set_bool("overviewActive", state.overview);
            self.shown = BarState {
                title: state.title.clone(),
                windows: state.windows,
                overview: state.overview,
            };
        }
        self.scene.set_real("clockSeconds", now.as_secs_f64());

        self.scene.advance(now);
        let rendered = match self.scene.render() {
            Ok(rendered) => rendered,
            Err(err) => {
                tracing::warn!(?err, "the bar did not render");
                return None;
            }
        };

        let size = (width, BAR_HEIGHT);
        let resized = self.buffer_size != size;
        if self.buffer.is_none() || resized {
            self.buffer = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
            self.buffer_size = size;
        }
        let buffer = self.buffer.as_mut()?;

        // Uploaded only when Qt actually redrew, so an idle bar costs a
        // comparison per frame rather than a copy and a texture upload. A fresh
        // buffer is empty, so a resize always uploads.
        if rendered.changed || resized {
            let mut context = buffer.render();
            // Copied row by row: Qt pads its rows to its own stride, which is
            // not necessarily the buffer's.
            let copy = context.draw(|target| {
                let row_bytes = usize::try_from(size.0.max(0)).unwrap_or_default() * 4;
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
                tracing::warn!("the bar image was smaller than its buffer");
                return None;
            }
        }

        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (0.0, 0.0),
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .inspect_err(|err| tracing::warn!(?err, "could not upload the bar"))
        .ok()
        .map(Element::Chrome)
    }

    /// Pointer input, in output coordinates. Returns whether the bar took it.
    pub(crate) fn pointer(&mut self, x: f64, y: f64, pressed: Option<bool>) -> bool {
        if y >= f64::from(BAR_HEIGHT) {
            return false;
        }
        self.scene.pointer(x, y, pressed);
        true
    }
}

/// Where the QML lives.
///
/// Overridable so the bar can be edited without rebuilding, which is most of
/// the point of authoring it in QML.
fn qml_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SOLIUM_QML_TOPBAR") {
        return PathBuf::from(path);
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/topbar.qml"))
}
