//! Window frames, drawn by the compositor from QML.
//!
//! One QML scene per decorated window, rendered by the same host as the bar —
//! see `qml.rs`. Each scene re-rasterises only when Qt says it changed, so a
//! screen full of idle windows costs a comparison per window per frame rather
//! than a rasterisation per window per frame. That is the whole reason this can
//! be one scene per window and not one shared atlas.
//!
//! **The frame reserves its height.** A window's *outer* rect is its client
//! rect grown upward by [`TITLEBAR_HEIGHT`], the client is placed below the
//! frame, and every presentation transform applies to the outer rect. Frame and
//! window therefore scale, move and animate as one object — in overview a
//! thumbnail carries its own titlebar — and the client area is never covered.

use std::{collections::HashMap, path::PathBuf, time::Duration};

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
    reexports::wayland_server::backend::ObjectId,
    utils::{Logical, Rectangle, Size, Transform},
};

use crate::qml;

/// How tall a window frame is, and so how much of a window's slot is not
/// client area.
pub(crate) const TITLEBAR_HEIGHT: i32 = 32;

/// What a frame button asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Close,
    ToggleMaximize,
}

impl Action {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "close" => Some(Self::Close),
            "maximize" => Some(Self::ToggleMaximize),
            _ => {
                tracing::warn!(name, "the titlebar asked for something unknown");
                None
            }
        }
    }
}

/// What a frame shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Shown {
    title: String,
    focused: bool,
}

/// One window's frame.
#[derive(Debug)]
pub(crate) struct Decoration {
    scene: qml::Scene,
    buffer: Option<MemoryRenderBuffer>,
    buffer_size: (i32, i32),
    shown: Shown,
    /// The scene has reported no further change, and nothing has been set on
    /// it since. Re-rendering it would produce the same pixels, so it is not
    /// re-rendered until something asks it to.
    quiet: bool,
    /// Pixels rendered but not yet copied into the buffer, because the buffer
    /// is created after the render: stride and rows, as the scene gave them.
    pending: Option<(usize, Vec<u8>)>,
    /// Where the window was before it was maximised. `Some` means maximised —
    /// one field rather than a flag and a rect that can disagree.
    pub(crate) restore: Option<Rectangle<i32, Logical>>,
}

impl Decoration {
    fn new(width: i32) -> Result<Self> {
        qml::start()?;
        let scene = qml::Scene::new(&qml_path(), width.max(1), TITLEBAR_HEIGHT)?;
        Ok(Self {
            scene,
            buffer: None,
            buffer_size: (0, 0),
            shown: Shown::default(),
            quiet: false,
            pending: None,
            restore: None,
        })
    }

    /// Draw the frame and return it as a render element.
    ///
    /// `rect` is where the frame is *drawn*, which in a mode is not where the
    /// window lives; the scene is always rasterised at its unscaled size and
    /// the element scales it, so a thumbnail's titlebar costs no more than a
    /// full-size one.
    pub(crate) fn frame<R>(
        &mut self,
        renderer: &mut R,
        rect: Rectangle<f64, Logical>,
        width: i32,
        title: &str,
        focused: bool,
        now: Duration,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let width = width.max(1);
        self.scene.resize(width, TITLEBAR_HEIGHT);

        // Compared field by field rather than by building a `Shown`: this runs
        // for every window of every frame, and the title is the one thing here
        // that allocates.
        if self.shown.title != title || self.shown.focused != focused {
            self.scene.set_string("title", title);
            self.scene.set_bool("focused", focused);
            self.shown.title.clear();
            self.shown.title.push_str(title);
            self.shown.focused = focused;
            self.quiet = false;
        }

        let size = (width, TITLEBAR_HEIGHT);
        let resized = self.buffer_size != size;
        if resized {
            self.quiet = false;
        }

        // A titlebar is animating only just after it was told something --
        // focus fading in, mostly. Once the scene says it has settled, driving
        // Qt's scene graph every frame buys identical pixels, so it stops
        // until the next thing is set on it.
        if !self.quiet {
            self.scene.advance(now);
            match self.scene.render() {
                Ok(rendered) => {
                    if rendered.changed || resized {
                        self.pending = Some((rendered.stride, rendered.pixels.to_vec()));
                    }
                    self.quiet = !rendered.changed;
                }
                Err(err) => {
                    tracing::warn!(?err, "a window frame did not render");
                    return None;
                }
            }
        }

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

        if let Some((stride, pixels)) = self.pending.take() {
            let mut context = buffer.render();
            let copy = context.draw(|target| {
                let row_bytes = usize::try_from(size.0.max(0)).unwrap_or_default() * 4;
                for (row, destination) in target.chunks_exact_mut(row_bytes).enumerate() {
                    let start = row * stride;
                    let Some(source) = pixels.get(start..start + row_bytes) else {
                        return Err(());
                    };
                    destination.copy_from_slice(source);
                }
                Ok(vec![Rectangle::from_size(size.into())])
            });
            if copy.is_err() {
                tracing::warn!("a frame image was smaller than its buffer");
                return None;
            }
        }

        // The drawn size scales the frame with the window it belongs to.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a frame is at most an output wide"
        )]
        let drawn: Size<i32, Logical> = (
            rect.size.w.round() as i32,
            rect.size.h.round().max(1.0) as i32,
        )
            .into();

        // `src` must be given whenever `size` is. Smithay defaults it to the
        // *drawn* size, which crops the buffer to its top-left corner instead
        // of scaling it — at full size the two are equal and it looks correct,
        // and it only shows up once a mode scales the frame down: the title
        // slides right and the buttons vanish off the edge.
        let source = Rectangle::from_size((f64::from(width), f64::from(TITLEBAR_HEIGHT)).into());

        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (rect.loc.x, rect.loc.y),
            buffer,
            None,
            Some(source),
            Some(drawn),
            Kind::Unspecified,
        )
        .inspect_err(|err| tracing::warn!(?err, "could not upload a window frame"))
        .ok()
    }

    /// Pointer input in frame-local coordinates.
    pub(crate) fn pointer(&mut self, x: f64, y: f64, pressed: Option<bool>) {
        self.scene.pointer(x, y, pressed);
    }

    /// Whether the pointer is over a button.
    ///
    /// Asked of QML rather than worked out from coordinates: QML owns the
    /// button layout, so a copy of it here would be a second authority that
    /// drifts the first time the frame is restyled.
    pub(crate) fn on_button(&self) -> bool {
        self.scene.get_bool("onButton")
    }

    /// What a button asked for since the last call, if anything.
    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.scene
            .take_string("action")
            .as_deref()
            .and_then(Action::parse)
    }
}

/// Every decorated window's frame.
///
/// Keyed by the toplevel's surface id, and *presence means decorated*: a client
/// that negotiated client-side decorations has no entry, so it draws its own
/// frame and the compositor draws none. Two frames on one window is what
/// happens when this is a flag instead of a lookup.
#[derive(Debug, Default)]
pub(crate) struct Decorations {
    frames: HashMap<ObjectId, Decoration>,
}

impl Decorations {
    /// Start decorating a window, if it is not decorated already.
    pub(crate) fn insert(&mut self, id: ObjectId, width: i32) {
        if self.frames.contains_key(&id) {
            return;
        }
        match Decoration::new(width) {
            Ok(decoration) => {
                self.frames.insert(id, decoration);
            }
            Err(err) => {
                // An undecorated window is worse than a decorated one and much
                // better than no window.
                tracing::error!(?err, "could not load a window frame, leaving it bare");
            }
        }
    }

    pub(crate) fn remove(&mut self, id: &ObjectId) {
        if self.frames.remove(id).is_some() {
            tracing::debug!("dropped a window frame");
        }
    }

    pub(crate) fn get_mut(&mut self, id: &ObjectId) -> Option<&mut Decoration> {
        self.frames.get_mut(id)
    }

    /// How many frames are being kept. For leak diagnostics: this should
    /// return to what it was once every window is closed.
    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn contains(&self, id: &ObjectId) -> bool {
        self.frames.contains_key(id)
    }
}

/// Where the frame's QML lives.
///
/// Overridable so a frame can be restyled and reloaded without a rebuild,
/// which is most of the point of authoring it in QML.
fn qml_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SOLIUM_QML_TITLEBAR") {
        return PathBuf::from(path);
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/titlebar.qml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_names_map_to_actions() {
        assert_eq!(Action::parse("close"), Some(Action::Close));
        assert_eq!(Action::parse("maximize"), Some(Action::ToggleMaximize));
        // Unknown names are ignored rather than guessed at: a typo in QML must
        // not close a window.
        assert_eq!(Action::parse("clos"), None);
        assert_eq!(Action::parse(""), None);
    }
}
