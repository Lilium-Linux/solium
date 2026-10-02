//! A hosted QML scene: one file the compositor draws. Every surface a script
//! declares with `sol.surface` is one — the wallpaper, the tweaks panel and a
//! hosted shell among them — and so is the scene a launched window shows
//! until its application draws.
//!
//! No opinion about what the scene *is*. It hosts one QML file, gives it the
//! area it was placed in, forwards pointer events, and rebuilds it when the
//! file changes. What appears is entirely the scene's business — the
//! compositor supplies a canvas, an event stream and a reload, and nothing
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
    utils::{Logical, Point, Rectangle, Transform},
};

use crate::{
    json::Json,
    qml::{
        self,
        hosted::{GrabReport, Hit, KeyboardReport, SceneKey, ScenePointer},
        paint::{Gpu, Placement},
    },
    render::{Drawn, Element},
    scripted::SceneReserve,
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
    /// The monitor the scene is hosted on, for a `sol.surface` instance; `None`
    /// for the loading window's scene.
    /// `scripted::tests::a_surface_instance_is_hosted_on_its_monitor`.
    monitor: Option<String>,
    newest: Option<SystemTime>,
    checked: Duration,
    /// The last hit asked of the scene.
    /// `tests::a_cached_hit_follows_the_scene_once_qt_has_run`.
    hit_cache: std::cell::Cell<Option<CachedHit>>,
    /// What the scene last said it reserves.
    /// `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`.
    scene_reserve: SceneReserve,
}

/// One hit the scene answered: the point in the scene's own coordinates, the
/// generation it was asked at, and the answer.
/// `tests::a_cached_hit_follows_the_scene_once_qt_has_run`.
#[derive(Clone, Copy, Debug)]
struct CachedHit {
    at: (f64, f64),
    generation: u64,
    hit: Hit,
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
        Self::with_monitor(source, properties, None)
    }

    /// Host the QML at `source` on one monitor: `Solium.monitor` inside it is
    /// that monitor.
    /// `qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`,
    /// `scripted::tests::a_surface_instance_is_hosted_on_its_monitor`.
    pub(crate) fn hosted(source: PathBuf, properties: &str, monitor: &str) -> Result<Self> {
        Self::with_monitor(source, properties, Some(monitor))
    }

    fn with_monitor(source: PathBuf, properties: &str, monitor: Option<&str>) -> Result<Self> {
        qml::start()?;
        // 1x1 and not the real size, which is not known until the first draw:
        // this is the same placeholder the software path has always built, and
        // on the GPU path it also means one wasted 1x1 buffer per surface
        // rather than a scene that does not exist until something asks for a
        // frame. Loading is where a bad QML path is worth reporting, and a
        // surface that has not built anything cannot report it.
        let scene = build(&source, properties, monitor, 1, 1)?;
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
            monitor: monitor.map(str::to_owned),
            newest,
            checked: Duration::ZERO,
            hit_cache: std::cell::Cell::new(None),
            scene_reserve: SceneReserve::default(),
        })
    }

    /// Deliver one pointer event, in compositor coordinates, to the scene
    /// drawn across `area`. Whether the scene should have it is the caller's
    /// question.
    /// `scripted::tests::a_delivered_press_reaches_the_instance_in_its_own_coordinates`,
    /// `qml::hosted::tests::a_right_press_reaches_a_mouse_area_as_the_right_button`.
    pub(crate) fn pointer(
        &mut self,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
        event: &ScenePointer,
    ) {
        let local = location - area.loc.to_f64();
        self.scene.pointer_event(local.x, local.y, event);
    }

    /// What the scene claims at a point in compositor coordinates. One item
    /// walk per point until Qt next runs: the press, the pointer's shape and
    /// the cursor's every-frame check all ask, and a still pointer is walked
    /// once. `qml::hosted::tests::the_item_tree_decides_what_a_point_claims`,
    /// `tests::a_cached_hit_follows_the_scene_once_qt_has_run`.
    pub(crate) fn hit(&self, area: Rectangle<i32, Logical>, location: Point<f64, Logical>) -> Hit {
        let local = location - area.loc.to_f64();
        let at = (local.x, local.y);
        let generation = qml::hosted::generation();
        if let Some(cached) = self.hit_cache.get()
            && cached.at == at
            && cached.generation == generation
        {
            return cached.hit;
        }
        let hit = self.scene.hit(local.x, local.y);
        self.hit_cache.set(Some(CachedHit {
            at,
            generation,
            hit,
        }));
        hit
    }

    /// The pointer left this scene. `qml::hosted::tests::a_left_scene_drops_its_hover`.
    pub(crate) fn leave(&mut self) {
        self.scene.leave();
    }

    /// Read what the scene reserves, if it said something new.
    /// `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`.
    pub(crate) fn take_reserve(&mut self) {
        if let Some(reserve) = self.scene.take_reserve() {
            self.scene_reserve = reserve;
        }
    }

    pub(crate) fn scene_reserve(&self) -> SceneReserve {
        self.scene_reserve
    }

    /// What the scene says of its grabs since it was last asked.
    /// `qml::hosted::tests::a_grab_is_held_while_active_and_dismissed_on_request`.
    pub(crate) fn take_grab(&mut self) -> GrabReport {
        self.scene.take_grab()
    }

    /// Whether a point in compositor coordinates is inside an active grab's
    /// target of the scene drawn across `area`.
    /// `tests::a_grab_target_is_asked_about_where_its_scene_is_drawn`,
    /// `qml::hosted::tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`.
    pub(crate) fn grab_contains(
        &self,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
    ) -> bool {
        let local = location - area.loc.to_f64();
        self.scene.grab_contains(local.x, local.y)
    }

    /// Dismiss the scene's grabs. `a_cached_hit_follows_a_dismissal`.
    pub(crate) fn dismiss(&mut self) {
        self.scene.dismiss();
    }

    /// What the scene says of its keyboard wants since it was last asked.
    /// `qml::hosted::tests::a_field_that_wants_the_keyboard_reports_its_claims`.
    pub(crate) fn take_keyboard(&mut self) -> KeyboardReport {
        self.scene.take_keyboard()
    }

    /// Tell the scene one key. `qml::hosted::tests::text_typed_on_russian_reaches_the_field`.
    pub(crate) fn key(&mut self, key: &SceneKey) {
        self.scene.key(key);
    }

    /// The compositor took the keyboard back.
    /// `qml::hosted::tests::a_field_that_wants_the_keyboard_reports_its_claims`.
    pub(crate) fn let_go_keyboard(&mut self) {
        self.scene.let_go_keyboard();
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

    /// Write `changed` into the live scene, and remember `bag`, the whole
    /// declaration rendered once by the caller, as what a reload of the QML
    /// builds with.
    /// `scripted::tests::a_redeclared_property_is_written_into_the_live_scene`,
    /// `scripted::tests::a_redeclaration_that_only_drops_keys_still_updates_the_rebuild_bag`.
    pub(crate) fn set_properties(&mut self, bag: &str, changed: &[(String, Json)]) {
        for (path, value) in changed {
            if !self.scene.set_json(path, value) {
                tracing::debug!(
                    property = path,
                    "the scene has no such property, or refused the value"
                );
            }
        }
        bag.clone_into(&mut self.properties);
    }

    #[cfg(test)]
    pub(crate) fn scene_for_test(&mut self) -> &mut qml::Scene {
        &mut self.scene
    }

    #[cfg(test)]
    pub(crate) fn properties_for_test(&self) -> &str {
        &self.properties
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
        // On the monitor it was first built for:
        // `tests::a_reloaded_scene_stays_hosted_on_its_monitor`.
        match build(
            &self.source,
            &self.properties,
            self.monitor.as_deref(),
            width,
            height,
        ) {
            Ok(scene) => {
                self.scene = scene;
                self.hit_cache.set(None);
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
    /// scene. Whether a surface is see-through is the compositor's business —
    /// it is a presentation transform, the same as where the surface is and how
    /// big. On the software path it could not be done inside anyway: Qt's
    /// software renderer repaints only what it thinks changed, onto the pixels
    /// already there, so a scene fading itself out paints each half-transparent
    /// frame over its own opaque previous one and never fades at all.
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
/// Delegates, and stays here only to carry the scene's properties: the choice
/// itself belongs to [`qml::Scene::for_host`], because it is the same choice
/// the window frames and the pointer have to make and three modules each making
/// it for themselves is the bug that sharing it fixed.
fn build(
    source: &Path,
    properties: &str,
    monitor: Option<&str>,
    width: i32,
    height: i32,
) -> Result<qml::Scene> {
    match monitor {
        Some(monitor) => qml::Scene::for_monitor(source, width, height, Some(properties), monitor),
        None => qml::Scene::for_host(source, width, height, Some(properties)),
    }
}

/// The newest modification time anywhere the scene's QML lives.
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::ShellSurface;
    use crate::qml::hosted::{Hit, PointerKind, ScenePointer};
    use crate::qml::qt_test::on_the_qt_thread;

    /// **A cached hit follows a dismissal**: the menu its `onDismissed`
    /// closed takes nothing at the point it took a press at a moment ago.
    #[test]
    fn a_cached_hit_follows_a_dismissal() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-surface-cached-dismissal");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem {\n    MouseArea { id: menu; width: 20; height: 20 }\n    Grab { target: menu; active: menu.visible; onDismissed: menu.visible = false }\n}\n",
            )
            .expect("writing the scene");
            let mut surface =
                ShellSurface::hosted(path, "{}", "cached-dismissal-1").expect("the scene builds");
            let area = smithay::utils::Rectangle::new((0, 0).into(), (400, 30).into());
            let point = smithay::utils::Point::from((10.0, 10.0));
            let open = surface.hit(area, point);
            surface.dismiss();
            let dismissed = surface.hit(area, point);
            drop(surface);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (open, dismissed),
                (Hit::Press, Hit::Nothing),
                "(the open menu, the place it was once dismissed)"
            );
        });
    }

    /// **A grab's target is asked about in the scene's own coordinates**: a
    /// scene drawn on a second monitor, from 1920,0, has the point 1935,15
    /// inside a menu at 10,10 of it, and 15,15 of the first monitor outside.
    #[test]
    fn a_grab_target_is_asked_about_where_its_scene_is_drawn() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-surface-grab-drawn-at");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem {\n    Rectangle { id: menu; x: 10; y: 10; width: 20; height: 10 }\n    Grab { target: menu; active: true }\n}\n",
            )
            .expect("writing the scene");
            let surface =
                ShellSurface::hosted(path, "{}", "grab-drawn-at-1").expect("the scene builds");
            let area = smithay::utils::Rectangle::new((1920, 0).into(), (400, 30).into());
            let inside = surface.grab_contains(area, smithay::utils::Point::from((1935.0, 15.0)));
            let outside = surface.grab_contains(area, smithay::utils::Point::from((15.0, 15.0)));
            drop(surface);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (inside, outside),
                (true, false),
                "(the menu, drawn on the second monitor; the same place of the first)"
            );
        });
    }

    /// **A hit is cached only until Qt next runs**: asked again at the same
    /// point, it is what the scene's items say now, after a property write
    /// that moved the button away, after a press that hid it, and after an
    /// edit that rebuilt the scene with the button elsewhere.
    #[test]
    fn a_cached_hit_follows_the_scene_once_qt_has_run() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-surface-cached-hit");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            let path = directory.join("Scene.qml");
            let scene = |x: i32| {
                format!(
                    "import QtQuick\nItem {{\n    property int at: {x}\n    MouseArea {{ x: parent.at; width: 20; height: 20; onPressed: visible = false }}\n}}\n"
                )
            };
            std::fs::write(&path, scene(0)).expect("writing the scene");
            let mut surface =
                ShellSurface::hosted(path.clone(), "{}", "cached-1").expect("the scene builds");
            let area = smithay::utils::Rectangle::new((100, 0).into(), (400, 30).into());
            let point = smithay::utils::Point::from((110.0, 10.0));
            assert_eq!(surface.hit(area, point), Hit::Press, "the premise");

            let at = |x: f64| [("at".to_owned(), crate::json::Json::Number(x))];
            surface.set_properties("{}", &at(40.0));
            assert_eq!(
                surface.hit(area, point),
                Hit::Nothing,
                "a property write moved the button away, and the hit did not follow"
            );
            surface.set_properties("{}", &at(0.0));
            assert_eq!(surface.hit(area, point), Hit::Press);
            surface.pointer(
                area,
                point,
                &ScenePointer {
                    kind: PointerKind::Press(0x1),
                    buttons: 0x1,
                    modifiers: 0,
                    time: 0,
                },
            );
            assert_eq!(
                surface.hit(area, point),
                Hit::Nothing,
                "a press hid the button, and the hit did not follow"
            );

            std::fs::write(&path, scene(0)).expect("writing the scene");
            let mut rebuilt =
                ShellSurface::hosted(path.clone(), "{}", "cached-2").expect("the scene builds");
            assert_eq!(rebuilt.hit(area, point), Hit::Press, "the premise");
            std::fs::write(&path, scene(40)).expect("editing the scene");
            std::fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(SystemTime::now() + Duration::from_secs(60)))
                .expect("dating the edit");
            rebuilt.reload_if_changed(Duration::from_secs(60), (400, 30));
            assert_eq!(
                rebuilt.hit(area, point),
                Hit::Nothing,
                "the edit rebuilt the scene with the button elsewhere, and the hit did not follow"
            );
            drop(surface);
            drop(rebuilt);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A cached hit follows what moves the scene's items without a write
    /// from the compositor**: a monitor row published, which a binding in the
    /// scene reads, and an `action` taken, whose clearing runs the scene's
    /// own handler.
    #[test]
    fn a_cached_hit_follows_published_rows_and_a_taken_action() {
        use crate::models::diff::{Row, diff, render};
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-surface-cached-rows");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            let path = directory.join("Scene.qml");
            std::fs::write(
                &path,
                r#"
                import QtQuick
                import Solium
                Item {
                    property string action: "go"
                    MouseArea { x: Solium.monitor.present ? 40 : 0; width: 20; height: 20 }
                    MouseArea { id: second; x: 100; width: 20; height: 20 }
                    onActionChanged: if (action === "") second.x = 140
                }
                "#,
            )
            .expect("writing the scene");
            let mut surface =
                ShellSurface::hosted(path, "{}", "cached-rows-1").expect("the scene builds");
            let area = smithay::utils::Rectangle::new((0, 0).into(), (400, 30).into());
            let at = |x: f64| smithay::utils::Point::from((x, 10.0));
            let first = surface.hit(area, at(10.0));

            let row = Row {
                key: "cached-rows-1".to_owned(),
                values: std::collections::BTreeMap::from([(
                    "name",
                    crate::json::Json::Text("cached-rows-1".to_owned()),
                )]),
            };
            assert!(crate::qml::hosted::apply_rows(
                crate::qml::hosted::Model::Monitors,
                &render(&diff(&[], &[row]))
            ));
            let published = surface.hit(area, at(10.0));
            let second = surface.hit(area, at(110.0));
            let taken = surface.taken_action();
            let after_the_action = surface.hit(area, at(110.0));
            drop(surface);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (first, published, second, taken.as_deref(), after_the_action),
                (
                    Hit::Press,
                    Hit::Nothing,
                    Hit::Press,
                    Some("go"),
                    Hit::Nothing
                ),
                "(the first button, there again once its monitor was published, the second, \
                 the action taken, the second's place after that)"
            );
        });
    }

    /// **A scene rebuilt for an edit stays on its monitor**: the reload builds
    /// it hosted on the same monitor it was first built for.
    #[test]
    fn a_reloaded_scene_stays_hosted_on_its_monitor() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-surface-reload-hosted");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            let path = directory.join("Scene.qml");
            let scene = |edition: i32| {
                format!(
                    "import QtQuick\nimport Solium\nItem {{\n    readonly property int edition: {edition}\n    readonly property int named: Solium.monitor.name === \"reload-1\" ? 1 : 0\n}}\n"
                )
            };
            std::fs::write(&path, scene(1)).expect("writing the scene");
            let mut surface =
                ShellSurface::hosted(path.clone(), "{}", "reload-1").expect("the scene builds");
            assert_eq!(surface.scene_for_test().get_int("named"), 1);

            std::fs::write(&path, scene(2)).expect("editing the scene");
            std::fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(SystemTime::now() + Duration::from_secs(60)))
                .expect("dating the edit");
            surface.reload_if_changed(Duration::from_secs(60), (16, 16));
            let scene = surface.scene_for_test();
            assert_eq!(scene.get_int("edition"), 2, "the edit was not reloaded");
            assert_eq!(
                scene.get_int("named"),
                1,
                "the reloaded scene is no longer hosted on its monitor"
            );
            let _ = std::fs::remove_dir_all(&directory);
        });
    }
}
