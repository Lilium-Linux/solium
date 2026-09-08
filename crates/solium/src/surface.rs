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
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Result, anyhow};
use smithay::{
    backend::{
        allocator::Fourcc,
        egl::fence::EGLFence,
        renderer::{
            ImportDma as _, Renderer as _,
            element::{
                Id, Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                texture::TextureRenderElement,
            },
            gles::{GlesRenderer, GlesTexture},
            sync::SyncPoint,
            utils::DamageBag,
        },
    },
    utils::{Buffer as BufferCoords, Logical, Rectangle, Transform},
};

use crate::{qml, render::Element};

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
    /// uploads.
    Memory(Option<MemoryRenderBuffer>),
    /// Rendered by Qt straight into a dmabuf we allocated, which the compositor
    /// imports and samples. The buffer belongs to the scene; this is only the
    /// texture it was imported as, kept so an idle frame costs no import.
    Gpu(Option<GlesTexture>),
}

/// One QML scene, hosted by the compositor.
#[derive(Debug)]
pub(crate) struct ShellSurface {
    scene: qml::Scene,
    backing: Backing,
    size: (i32, i32),
    /// Stable for the life of the surface, so the damage tracker sees one
    /// element moving and changing rather than a new one every frame.
    ///
    /// Only the GPU path needs it: a memory element takes its identity from the
    /// buffer, which lives across frames on its own.
    id: Id,
    /// What of the imported texture has changed, in its own pixels.
    ///
    /// The GPU path's answer to a question the memory path never has to ask.
    /// Qt draws into the same buffer every frame, so nothing about the texture
    /// says whether it holds anything new; without this the element is either
    /// permanently undamaged — a shell frozen on its first frame — or
    /// permanently new, which repaints its whole area on every frame anything
    /// else draws.
    damage: DamageBag<i32, BufferCoords>,
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
                Backing::Gpu(None)
            } else {
                Backing::Memory(None)
            },
            size: (0, 0),
            id: Id::new(),
            damage: DamageBag::default(),
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

        // 1x1 on the software path, as it always was: the next draw resizes
        // it. A GPU scene cannot be resized -- its pixel size is its buffer's
        // -- so rebuilding one at 1x1 would only have it rebuilt again at the
        // real size on the next line of `render_on_gpu`, two whole Qt scenes
        // and two buffers for one edit. The size is known here.
        let (width, height) = match self.backing {
            Backing::Memory(_) => (1, 1),
            Backing::Gpu(_) => (self.size.0.max(1), self.size.1.max(1)),
        };
        match build(&self.source, &self.properties, width, height) {
            Ok(scene) => {
                self.scene = scene;
                match &mut self.backing {
                    Backing::Memory(buffer) => {
                        *buffer = None;
                        self.size = (0, 0);
                    }
                    Backing::Gpu(texture) => {
                        // A new buffer, so the old texture names the old one.
                        *texture = None;
                        self.size = (width, height);
                        // Nothing in the new buffer is the old buffer's, so no
                        // damage since then means anything.
                        self.damage.reset();
                    }
                }
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
    pub(crate) fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        area: Rectangle<i32, Logical>,
        now: Duration,
        alpha: f32,
        scale: f64,
    ) -> Option<Element> {
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

        if matches!(self.backing, Backing::Gpu(_)) {
            return self
                .on_gpu(renderer, area, now, alpha, scale, size)
                .map(Element::Screen);
        }
        self.reload_if_changed(now);
        self.in_memory(renderer, area, alpha, scale, size)
            .map(Element::Chrome)
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

        let resized = self.size != size;
        let Backing::Memory(slot) = &mut self.backing else {
            return None;
        };
        if slot.is_none() || resized {
            *slot = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
            self.size = size;
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

    /// The GPU path: Qt draws into a buffer we allocated, and we sample it.
    ///
    /// Three things have to happen in this order and none of them are optional.
    /// Qt renders; the compositor's EGL context goes back on this thread;
    /// Qt's fence is waited for. The second is why the Qt half is one call
    /// rather than inline — see [`restore`] — and the third is why the fence is
    /// imported rather than dropped: sampling a buffer that is still being
    /// written is a race that surfaces as garbage on maybe one frame in
    /// several hundred, which is the hardest possible thing to attribute.
    fn on_gpu(
        &mut self,
        renderer: &mut GlesRenderer,
        area: Rectangle<i32, Logical>,
        now: Duration,
        alpha: f32,
        scale: f64,
        size: (i32, i32),
    ) -> Option<TextureRenderElement<GlesTexture>> {
        let rendered = self.render_on_gpu(now, size, scale);
        // Unconditional, and underneath every way out of the call above,
        // including the paths that failed. Rendering leaves Qt's context on the
        // thread; building a scene and freeing one leave none. Neither is a
        // state the next line can run in — `EGLFence::import` is an
        // `eglCreateSync` of our own, not smithay's, so nothing will make our
        // context current for it.
        if let Err(err) = restore(renderer) {
            tracing::error!(?err, "the compositor's EGL context could not be restored");
            return None;
        }
        let rendered = match rendered {
            Ok(rendered) => rendered,
            Err(err) => {
                tracing::warn!(?err, "the shell surface did not render on the GPU");
                return None;
            }
        };

        // `None` is "Qt had nothing new to draw", so the texture already in
        // hand is this frame's picture and no import and no wait are owed.
        if let Some(fence) = rendered {
            // A fence inside the `Some` is the driver's; without one the host
            // has already waited on the CPU with `glFinish` and the frame is
            // complete, which is a correct answer and not a missing fence.
            if let Some(fence) = fence
                && let Err(err) = wait_for(renderer, fence)
            {
                // Not drawing this frame is the cheaper wrong answer: sampling
                // anyway is the race described above.
                tracing::warn!(?err, "could not wait for Qt's fence; skipping a frame");
                return None;
            }
            // Re-imported every frame rather than once: `import_dmabuf` is
            // cached on the buffer and re-binds the EGLImage to the same
            // texture name, which is what makes what Qt just wrote visible to
            // our context.
            let imported = match self.scene.buffer() {
                Some(buffer) => renderer.import_dmabuf(buffer, None),
                None => {
                    tracing::warn!("a GPU shell surface has no buffer to sample");
                    return None;
                }
            };
            match imported {
                Ok(texture) => {
                    // Whole-buffer damage, because Qt does not say what it
                    // repainted and the buffer is the same one every frame.
                    self.damage
                        .add([Rectangle::from_size((size.0, size.1).into())]);
                    self.backing = Backing::Gpu(Some(texture));
                }
                Err(err) => {
                    tracing::warn!(?err, "could not import the shell surface's buffer");
                    return None;
                }
            }
        }

        let Backing::Gpu(Some(texture)) = &self.backing else {
            // Reachable only before the first successful render: Qt reported a
            // scene it has never drawn as up to date, or the import failed and
            // the next frame will try again.
            return None;
        };

        // As on the software path: `src` is the whole buffer in its own pixels,
        // `size` is the logical destination the output scale takes back up to
        // exactly those pixels, and the position is physical.
        let source = Rectangle::from_size((f64::from(size.0), f64::from(size.1)).into());
        Some(TextureRenderElement::from_texture_with_damage(
            self.id.clone(),
            renderer.context_id(),
            (f64::from(area.loc.x) * scale, f64::from(area.loc.y) * scale),
            texture.clone(),
            1,
            Transform::Normal,
            Some(alpha),
            Some(source),
            Some(area.size),
            None,
            self.damage.snapshot(),
            Kind::Unspecified,
        ))
    }

    /// Everything that hands this thread's GL context to Qt, in one place.
    ///
    /// Gathered into one call so [`ShellSurface::on_gpu`] can put the context
    /// back underneath it on every path out, the failures included. Nothing in
    /// here may touch the renderer.
    fn render_on_gpu(
        &mut self,
        now: Duration,
        size: (i32, i32),
        scale: f64,
    ) -> Result<Option<Option<OwnedFd>>> {
        self.reload_if_changed(now);

        if self.size != size {
            // A dmabuf cannot be resized and a GPU scene's pixel size is its
            // buffer's, so changing size means a new buffer and a new scene.
            // The old one is dropped by the assignment, after the new one
            // exists, which is what leaves a surface whose rebuild failed still
            // drawing last frame's picture rather than nothing at all.
            self.scene = build(&self.source, &self.properties, size.0, size.1)?;
            self.size = size;
            self.backing = Backing::Gpu(None);
            self.damage.reset();
        }
        // Only the ratio can have moved; the pixel size is the buffer's and the
        // host refuses to change it.
        self.scene.resize(size.0, size.1, scale);
        self.scene.render_gpu()
    }
}

/// One scene of the given pixel size, on whichever path Qt came up on.
///
/// Not a preference. Qt fixes its scene graph inside `QGuiApplication` and a
/// host that came up on one backend refuses scenes of the other kind, so this
/// reads what Qt did rather than deciding anything.
fn build(source: &Path, properties: &str, width: i32, height: i32) -> Result<qml::Scene> {
    if qml::on_gpu() {
        qml::Scene::gpu_sized(source, width, height, Some(properties))
    } else {
        qml::Scene::with_properties(source, width, height, Some(properties))
    }
}

/// Put the compositor's EGL context back after Qt has had the thread.
///
/// `render_gpu` leaves Qt's context on the thread and building or freeing a
/// scene leaves none, so after any of them the thread is not in a state this
/// file can make an EGL call in.
///
/// Only *this* file, and that distinction is the whole reason this exists as a
/// deliberate call rather than something the renderer handles. `GlesRenderer`
/// re-binds its own context inside every operation it offers, so an empty
/// thread costs it one `eglMakeCurrent` and nothing else. What it cannot cover
/// is a call that is not smithay's, and the very next thing here is exactly
/// that: `EGLFence::import` is an `eglCreateSync` against our display, and it
/// needs a current context nobody else is going to make for it.
///
/// `EGLContext::make_current` is the API. There is no `bind_context`.
///
/// This has a matching half on the other side, and neither works alone. An
/// `eglMakeCurrent` is invisible to Qt — it keeps its own thread-local record of
/// which context is current — so once this has run, Qt believes it still has the
/// thread and skips the `makeCurrent` its next call needs. On a render that
/// draws the frame into our context and leaves the buffer empty; on a *teardown*
/// it deletes Qt's GL object names out of our context, which are our objects.
/// See `clear_stale_current_context` in `qml/host.cpp`, which is what makes the
/// second frame draw and the first free safe.
#[expect(unsafe_code, reason = "restoring our EGL context after Qt")]
fn restore(renderer: &GlesRenderer) -> Result<()> {
    // SAFETY: called on the thread that owns this context, with no other
    // context of ours in use on it. What makes it unsafe is that the context
    // could have been destroyed; Qt has its own and does not touch this one.
    unsafe { renderer.egl_context().make_current() }
        .map_err(|err| anyhow!("making the compositor's EGL context current again: {err}"))
}

/// Wait for Qt's frame to land before sampling the buffer it landed in.
///
/// `Renderer::wait` takes a `SyncPoint` and not a raw fd, so the fence is
/// imported first — `EGLFence::import` is the only constructor that takes a
/// native fence fd. Smithay then inserts it into our context if it can and
/// blocks the thread on it if it cannot, so a driver with no server-side wait
/// costs a stall rather than correctness.
fn wait_for(renderer: &mut GlesRenderer, fence: OwnedFd) -> Result<()> {
    let imported = {
        let display = renderer.egl_context().display();
        EGLFence::import(display, fence).map_err(|err| anyhow!("importing Qt's fence: {err}"))?
    };
    renderer
        .wait(&SyncPoint::from(imported))
        .map_err(|err| anyhow!("waiting on Qt's fence: {err}"))
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
