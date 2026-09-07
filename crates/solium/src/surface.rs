//! A shell surface: the compositor drawing QML that is not its own.
//!
//! No opinion about what the shell *is*. It hosts one QML file, gives it the
//! whole output to place itself in, forwards every pointer event, and rebuilds
//! it when the file changes. What appears is entirely the shell's business —
//! the compositor supplies a canvas, an event stream and a reload, and nothing
//! else.
//!
//! That boundary is deliberate. The first version of this was a dock, with
//! icons and a layout the compositor decided, and it was wrong twice over: it
//! was not the shell's dock, and it meant the compositor had opinions about
//! docks. A host has no opinions.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

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
    utils::{Logical, Rectangle, Transform},
};

use crate::qml;

/// How often the QML is checked for edits.
///
/// Shell work is edit-look-edit, and restarting a compositor to see a colour
/// change is enough friction that nobody tunes anything. Twice a second is
/// under what anyone notices and over what a directory scan costs.
const RELOAD_INTERVAL: Duration = Duration::from_millis(500);

/// One QML scene, hosted by the compositor.
#[derive(Debug)]
pub(crate) struct ShellSurface {
    scene: qml::Scene,
    buffer: Option<MemoryRenderBuffer>,
    size: (i32, i32),
    source: PathBuf,
    properties: String,
    newest: Option<SystemTime>,
    checked: Duration,
}

impl ShellSurface {
    /// Host the QML at `source`, handing it `properties` as a JSON object.
    ///
    /// The properties matter: shell components declare things `required`, and
    /// a required property must be supplied before the component is built or
    /// it is never built at all.
    pub(crate) fn new(source: PathBuf, properties: &str) -> Result<Self> {
        qml::start()?;
        let scene = qml::Scene::with_properties(&source, 1, 1, Some(properties))?;
        let newest = newest_change(&source);
        Ok(Self {
            scene,
            buffer: None,
            size: (0, 0),
            source,
            properties: properties.to_owned(),
            newest,
            checked: Duration::ZERO,
        })
    }

    /// Pointer input, in compositor coordinates.
    ///
    /// Everything reaches the scene. A shell surface that hides itself — a
    /// dock against an edge, a panel that slides — has to watch the whole
    /// screen to know when to appear, so filtering by what is currently drawn
    /// would mean it could only be revealed by a pointer that was already
    /// over it.
    pub(crate) fn pointer(
        &mut self,
        area: Rectangle<i32, Logical>,
        x: f64,
        y: f64,
        pressed: Option<bool>,
    ) -> bool {
        let surface = area.to_f64();
        if !surface.contains((x, y)) {
            return false;
        }
        self.scene
            .pointer(x - surface.loc.x, y - surface.loc.y, pressed);
        true
    }

    /// Set a whole-number property on the scene.
    pub(crate) fn set_int(&mut self, name: &str, value: i32) {
        self.scene.set_int(name, value);
    }

    /// Take whatever the scene asked for, clearing it.
    ///
    /// The same one-way channel the window frames use: QML sets `action`, the
    /// compositor takes it and clears it, so a press is acted on once.
    pub(crate) fn taken_action(&mut self) -> Option<String> {
        self.scene.take_string("action").filter(|it| !it.is_empty())
    }

    /// Rebuild the scene if the QML changed on disk.
    ///
    /// The whole scene, because QML cannot apply an edit to a live object
    /// tree. State the shell was holding is lost, which is the honest cost of
    /// reloading.
    fn reload_if_changed(&mut self, now: Duration) {
        if now.saturating_sub(self.checked) < RELOAD_INTERVAL {
            return;
        }
        self.checked = now;

        let newest = newest_change(&self.source);
        if newest == self.newest {
            return;
        }
        self.newest = newest;

        // Without this the engine hands back what it compiled last time: the
        // reload runs, nothing throws, and the screen does not change.
        qml::clear_cache();

        match qml::Scene::with_properties(&self.source, 1, 1, Some(&self.properties)) {
            Ok(scene) => {
                self.scene = scene;
                self.buffer = None;
                self.size = (0, 0);
                tracing::info!("shell reloaded");
            }
            // The old scene keeps drawing: an edit that does not parse should
            // leave what you were looking at alone.
            Err(err) => tracing::warn!(?err, "reload failed, keeping the last scene"),
        }
    }

    /// Draw the surface across `area`, at `alpha`.
    ///
    /// The alpha is applied when the buffer reaches the screen, not inside the
    /// scene. It cannot be done inside: Qt's software renderer repaints only
    /// what it thinks changed, onto the pixels already there, so a scene fading
    /// itself out paints each half-transparent frame over its own opaque
    /// previous one and never fades at all. Whether a surface is see-through is
    /// the compositor's business anyway — it is a presentation transform, the
    /// same as where the surface is and how big.
    pub(crate) fn element<R>(
        &mut self,
        renderer: &mut R,
        area: Rectangle<i32, Logical>,
        now: Duration,
        alpha: f32,
        scale: f64,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.reload_if_changed(now);

        // In device pixels, as `Decoration::frame` does and for the same
        // reason: QML rasterises in pixels, so a scene drawn at its logical
        // size on a 2x screen is drawn at half that screen's resolution and
        // stretched.
        let pixels = |logical: i32| {
            #[expect(clippy::cast_possible_truncation, reason = "a scene on this screen")]
            let scaled = (f64::from(logical) * scale).round() as i32;
            scaled.max(1)
        };
        let size = (pixels(area.size.w), pixels(area.size.h));
        self.scene.resize(size.0, size.1, scale);

        let rendered = match self.scene.render() {
            Ok(rendered) => rendered,
            Err(err) => {
                tracing::warn!(?err, "the shell surface did not render");
                return None;
            }
        };

        let resized = self.size != size;
        if self.buffer.is_none() || resized {
            self.buffer = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
            self.size = size;
        }
        let buffer = self.buffer.as_mut()?;

        if rendered.changed || resized {
            let mut context = buffer.render();
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
                tracing::warn!("the shell image was smaller than its buffer");
                return None;
            }
        }

        // `src` is the whole buffer in its own pixels and `size` is the
        // logical destination, which the output scale then takes back up to
        // exactly these pixels. The position is physical.
        let source = Rectangle::from_size((f64::from(size.0), f64::from(size.1)).into());
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (f64::from(area.loc.x) * scale, f64::from(area.loc.y) * scale),
            buffer,
            Some(alpha),
            Some(source),
            Some(area.size),
            Kind::Unspecified,
        )
        .inspect_err(|err| tracing::warn!(?err, "could not upload the shell surface"))
        .ok()
    }
}

/// The newest modification time anywhere the shell's QML lives.
fn newest_change(source: &Path) -> Option<SystemTime> {
    fn newest_in(directory: &Path, best: &mut Option<SystemTime>, depth: usize) {
        if depth > 4 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                newest_in(&path, best, depth + 1);
            } else if path.extension().is_some_and(|kind| kind == "qml")
                && let Ok(time) = entry.metadata().and_then(|data| data.modified())
                && best.is_none_or(|current| time > current)
            {
                *best = Some(time);
            }
        }
    }

    let mut newest = std::fs::metadata(source)
        .and_then(|data| data.modified())
        .ok();
    // The tree it came from, when one is named: editing a widget three
    // directories away is still editing the shell.
    if let Some(root) = std::env::var_os("SOLIUM_SHELL_WATCH") {
        newest_in(Path::new(&root), &mut newest, 0);
    } else if let Some(parent) = source.parent() {
        newest_in(parent, &mut newest, 0);
    }
    newest
}
