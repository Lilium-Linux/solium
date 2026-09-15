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
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    utils::{Logical, Rectangle, Transform},
};

use crate::{
    qml::{
        self,
        paint::{Gpu, Placement},
    },
    render::{Drawn, Element},
};

/// How often the QML is checked for edits.
///
/// Shell work is edit-look-edit, and restarting a compositor to see a colour
/// change is enough friction that nobody tunes anything. Twice a second is
/// under what anyone notices and over what a directory scan costs.
const RELOAD_INTERVAL: Duration = Duration::from_millis(500);

/// How a rendered scene reaches the screen.
///
/// Which one a surface gets is not a preference: Qt fixes its scene graph for
/// the life of the process, a host that came up on one backend refuses scenes
/// of the other kind, and so this follows `qml::on_gpu` rather than choosing.
#[derive(Debug)]
enum Backing {
    /// Rasterised on the CPU into a shared-memory buffer the compositor
    /// uploads. The size is the buffer's, kept here because it is the memory
    /// path's alone — on the GPU path the buffer's size is [`Gpu`]'s business.
    Memory {
        buffer: Option<MemoryRenderBuffer>,
        size: (i32, i32),
    },
    /// Rendered by Qt straight into a dmabuf we allocated, which the compositor
    /// imports and samples. See `qml::paint`.
    Gpu(Gpu),
}

/// One QML scene, hosted by the compositor.
#[derive(Debug)]
pub(crate) struct ShellSurface {
    scene: qml::Scene,
    backing: Backing,
    source: PathBuf,
    properties: String,
    newest: Option<SystemTime>,
    checked: Duration,
}

/// A hosted scene animates on a clock of its own and damages nothing the
/// compositor can see, so a frame it has not been asked for is a frame it does
/// not get. See [`crate::render::Drawn`] for what that looked like.
impl crate::render::Painted for ShellSurface {
    fn something_new_to_draw(&self) -> bool {
        self.scene.needs_render()
    }

    fn animation_in_flight(&self) -> bool {
        self.scene.animation_in_flight()
    }
}

impl ShellSurface {
    /// Host the QML at `source`, handing it `properties` as a JSON object.
    ///
    /// The properties matter: shell components declare things `required`, and
    /// a required property must be supplied before the component is built or
    /// it is never built at all.
    ///
    /// Takes no renderer, and on the GPU path that is the reason the host gives
    /// the thread back itself rather than leaving the caller to restore a
    /// context. This is reached from a client attaching — `state.rs`'s
    /// `begin_loading` — with no frame in sight and nothing to restore *to*.
    /// See `Scene::gpu`.
    pub(crate) fn new(source: PathBuf, properties: &str) -> Result<Self> {
        qml::start()?;
        // 1x1 and not the real size, which is not known until the first draw:
        // this is the same placeholder the software path has always built, and
        // on the GPU path it also means one wasted 1x1 buffer per surface
        // rather than a scene that does not exist until something asks for a
        // frame. Loading is where a bad QML path is worth reporting, and a
        // surface that has not built anything cannot report it.
        let scene = build(&source, properties, 1, 1)?;
        let newest = newest_change(&source);
        Ok(Self {
            scene,
            backing: if qml::on_gpu() {
                // `(0, 0)` and not the 1x1 the scene really is, so the first
                // frame takes the rebind branch and lands on a buffer of the
                // size it is actually drawn at. A 1x1 picture stretched over an
                // output would otherwise be the first thing on screen.
                Backing::Gpu(Gpu::new((0, 0)))
            } else {
                Backing::Memory {
                    buffer: None,
                    size: (0, 0),
                }
            },
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
    fn reload_if_changed(&mut self, now: Duration, wanted: (i32, i32)) {
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

        // 1x1 on the software path, as it always was: the next draw resizes
        // it. A GPU scene cannot be resized -- its pixel size is its buffer's
        // -- so rebuilding one at 1x1 would only have it rebound again at the
        // real size on the next line of `Gpu::render`, two whole Qt scenes and
        // two buffers for one edit. The size the caller is about to ask for is
        // known here, which is why it is passed in.
        let (width, height) = match self.backing {
            Backing::Memory { .. } => (1, 1),
            Backing::Gpu(_) => (wanted.0.max(1), wanted.1.max(1)),
        };
        match build(&self.source, &self.properties, width, height) {
            Ok(scene) => {
                self.scene = scene;
                self.backing = match self.backing {
                    Backing::Memory { .. } => Backing::Memory {
                        buffer: None,
                        size: (0, 0),
                    },
                    // A new buffer, so the old texture names the old one and no
                    // damage recorded against it means anything.
                    Backing::Gpu(_) => Backing::Gpu(Gpu::new((width, height))),
                };
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
    ///
    /// Concrete on `GlesRenderer` rather than generic since the GPU path
    /// arrived. It has to be: taking the thread's EGL context back off Qt is
    /// `EGLContext::make_current`, and the only way to the context is
    /// `GlesRenderer::egl_context` — there is nothing on the `Renderer` traits
    /// that says it. `render.rs` keeps the traits; this file is already the
    /// place that knows what a dmabuf and an EGL fence are.
    ///
    /// Returns whether the scene is still animating as well as what to draw.
    /// A scripted surface animates on its own clock exactly as a decoration
    /// does -- a clock in a bar, a dock icon easing under the pointer, a
    /// notification sliding in -- and nothing it does damages the screen, so
    /// the next frame has to be asked for or it stops where it stands. See
    /// [`crate::render::Drawn`].
    pub(crate) fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        area: Rectangle<i32, Logical>,
        now: Duration,
        alpha: f32,
        scale: f64,
    ) -> Drawn {
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

        // Before either path, and before anything is bound: a reload builds a
        // whole new scene, which on the GPU path takes the thread off to Qt.
        // See `qml::no_frame_in_flight`.
        self.reload_if_changed(now, size);

        // And before the draw, which is what spends the flag. A scene that has
        // just been reloaded reads as having something to draw, which it does.
        Drawn::drawing(self, |this| {
            // Two fields of one struct, borrowed at once: the scene is what
            // renders and the backing is what holds the result.
            let Self { scene, backing, .. } = this;
            match backing {
                Backing::Gpu(gpu) => gpu
                    .element(
                        scene,
                        renderer,
                        size,
                        scale,
                        Placement {
                            position: (
                                f64::from(area.loc.x) * scale,
                                f64::from(area.loc.y) * scale,
                            ),
                            size: area.size,
                            alpha,
                            kind: Kind::Unspecified,
                        },
                    )
                    .map(Element::Screen),
                Backing::Memory { .. } => this
                    .in_memory(renderer, area, alpha, scale, size)
                    .map(Element::Chrome),
            }
        })
    }

    /// The software path, unchanged: Qt rasterises into a `QImage` and the
    /// compositor uploads it.
    fn in_memory(
        &mut self,
        renderer: &mut GlesRenderer,
        area: Rectangle<i32, Logical>,
        alpha: f32,
        scale: f64,
        size: (i32, i32),
    ) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
        self.scene.resize(size.0, size.1, scale);

        let rendered = match self.scene.render() {
            Ok(rendered) => rendered,
            Err(err) => {
                tracing::warn!(?err, "the shell surface did not render");
                return None;
            }
        };

        let Backing::Memory {
            buffer: slot,
            size: held,
        } = &mut self.backing
        else {
            return None;
        };
        let resized = *held != size;
        if slot.is_none() || resized {
            *slot = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
            *held = size;
        }
        let buffer = slot.as_mut()?;

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

/// One scene of the given pixel size, on whichever path Qt came up on.
///
/// Delegates, and stays here only to carry the shell's properties: the choice
/// itself belongs to [`qml::Scene::for_host`], because it is the same choice
/// the window frames and the pointer have to make and three modules each making
/// it for themselves is the bug that task fixed.
fn build(source: &Path, properties: &str, width: i32, height: i32) -> Result<qml::Scene> {
    qml::Scene::for_host(source, width, height, Some(properties))
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
