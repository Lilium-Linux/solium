//! The pointer's own scene: `cursor.scene`.
//!
//! A QML file the configuration names, found as `shell.scene` is
//! (`super::theme::tests::a_configured_scene_is_found_as_the_shells_is`), and
//! drawn for every named shape in place of the theme. It is configured the
//! way the shell is, in the same engine, and says where its point is with
//! `Solium.cursor.hotspot` on its root.
//!
//! Three things make it different from the floor, `qml/cursor.qml`, which is
//! still what a session with no scene and no theme draws:
//!
//! - **It is the size it says it is.** Its root's width and height, at most
//!   [`LARGEST`] logical pixels a side, so a glow or a shadow can reach past
//!   `cursor.size` with the hotspot still on the pointer; a root that sets
//!   neither is `cursor.size` square
//!   (`tests::the_scene_is_drawn_at_its_own_size_with_its_hotspot_on_the_pointer`).
//! - **It animates.** The floor is drawn once per size, which the module
//!   header of `cursor.rs` says an animation would freeze. This is drawn
//!   through [`Drawn`], so a running animation asks for the next frame on the
//!   one clock and a scene at rest asks for none
//!   (`tests::an_animating_scene_asks_for_the_next_frame_only_while_it_animates`).
//! - **Its plane is a predicate, [`plane`].** Its picture is still a
//!   `MemoryRenderBuffer` on both paths, so it can go on the hardware cursor
//!   plane as the floor's does; a scene that reads the backdrop would be
//!   composited instead, and nothing can until materials exist (#199)
//!   (`tests::a_scene_with_no_material_is_a_cursor_plane_element`).
//!
//! What animating costs on the GPU path is the readback `super::Backing`
//! measures, paid on every frame the picture changes rather than once per
//! size: tens of microseconds at pointer sizes, by `dev/wirecheck`'s sweep. A
//! scene at rest pays nothing, since nothing is drawn again.

use std::path::Path;

use anyhow::Result;
use smithay::{
    backend::renderer::{
        element::{Kind, memory::MemoryRenderBufferRenderElement},
        gles::GlesRenderer,
    },
    utils::{Logical, Point, Rectangle},
};

use super::{Backing, Cursor, KEPT};
use crate::{
    qml::{
        self,
        paint::{Gpu, Kept, Said},
    },
    render::{Drawn, Element, Painted},
};

/// The most a side of the scene may be, in logical pixels: `cursor.size`'s
/// own ceiling, so a scene cannot ask for a larger buffer than a setting can.
/// `tests::the_scene_is_drawn_at_its_own_size_with_its_hotspot_on_the_pointer`.
pub(crate) const LARGEST: i32 = 256;

/// The highest scale a side is multiplied by, as `theme::pixels` bounds it.
const MAX_SCALE: f64 = 8.0;

/// A scene that animates asks for frames; one at rest costs nothing. See
/// [`Drawn`]. `tests::an_animating_scene_asks_for_the_next_frame_only_while_it_animates`.
impl Painted for Cursor {
    fn something_new_to_draw(&self) -> bool {
        self.scene.needs_render()
    }

    fn animation_in_flight(&self) -> bool {
        self.scene.animation_in_flight()
    }
}

impl Cursor {
    /// The scene at `source`, sized by its own root, which is `size` logical
    /// pixels square when it sets no size of its own.
    /// `tests::the_scene_is_drawn_at_its_own_size_with_its_hotspot_on_the_pointer`.
    pub(crate) fn configured(source: &Path, size: i32) -> Result<Self> {
        qml::start()?;
        let scene = qml::Scene::sized_by_root(source, size)?;
        Ok(Self {
            scene,
            backing: if qml::on_gpu() {
                Backing::Gpu(Gpu::new((size, size)))
            } else {
                Backing::Memory
            },
            size,
            buffers: Kept::keeping(KEPT),
            drawing: Said::default(),
            uploading: Said::default(),
        })
    }

    /// How big the scene says it is, in logical pixels, each side between one
    /// and [`LARGEST`].
    /// `tests::the_scene_is_drawn_at_its_own_size_with_its_hotspot_on_the_pointer`.
    fn own_size(&self) -> (i32, i32) {
        let (width, height) = self.scene.root_size();
        (side(width), side(height))
    }

    /// The point of the picture that sits on the pointer, in its logical
    /// pixels. `tests::the_scene_is_drawn_at_its_own_size_with_its_hotspot_on_the_pointer`.
    fn hotspot(&self) -> Point<f64, Logical> {
        self.scene.cursor_hotspot().into()
    }

    /// Whether any item of the scene reads the backdrop, which is what would
    /// take it off the cursor plane.
    ///
    /// Nothing can until materials exist (#199): every item reads
    /// `Solium.materialState` as `"off"`, and `Solium.material` is accepted and
    /// read by nothing, so this is `false` for every scene. When materials
    /// arrive, a material that is visible is the scene's answer here, and
    /// [`plane`] already says what follows from it.
    /// `tests::a_scene_with_no_material_is_a_cursor_plane_element`.
    #[expect(
        clippy::unused_self,
        reason = "the seam #199 fills: the scene's own answer, once a material can be visible"
    )]
    const fn reads_backdrop(&self) -> bool {
        false
    }

    /// The `Kind` of this scene's picture. `tests::a_scene_with_no_material_is_a_cursor_plane_element`.
    fn kind(&self) -> Kind {
        plane(self.reads_backdrop())
    }

    /// The scene as something to draw at `location` on an output at `scale`,
    /// and whether it is still moving: read before the draw, as [`Drawn`]
    /// requires.
    /// `tests::an_animating_scene_asks_for_the_next_frame_only_while_it_animates`.
    pub(crate) fn drawn(
        &mut self,
        renderer: &mut GlesRenderer,
        location: Point<f64, Logical>,
        scale: f64,
    ) -> Drawn {
        Drawn::drawing(self, |this| this.placed(renderer, location, scale))
    }

    /// The picture, uploaded and placed with its hotspot on the pointer.
    fn placed(
        &mut self,
        renderer: &mut GlesRenderer,
        location: Point<f64, Logical>,
        scale: f64,
    ) -> Option<Element> {
        let size = self.own_size();
        let held = self.picture(Some(&mut *renderer), device(size, scale), scale)?;
        let position = origin(location, self.hotspot(), scale);
        let kind = self.kind();
        let buffer = self.buffers.get_mut(held)?;
        // As the floor does: the whole buffer in its own pixels, drawn at the
        // scene's logical size, which the output's scale takes back up to the
        // same pixels, as `copy_element_to_cursor_bo` requires of the plane.
        let source = Rectangle::from_size((f64::from(held.0), f64::from(held.1)).into());
        let uploaded = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            buffer,
            None,
            Some(source),
            Some(size.into()),
            kind,
        );
        match uploaded {
            Ok(element) => {
                self.uploading.worked();
                Some(Element::Chrome(element))
            }
            Err(err) => {
                self.uploading.once(|| {
                    tracing::warn!(
                        ?err,
                        "could not upload the pointer's scene; drawing the theme or Solium's own \
                         pointer instead. Said once until it uploads again"
                    );
                });
                None
            }
        }
    }

    /// The picture at `wanted` device pixels: the one kept when Qt has
    /// nothing new, and a new one when it has. Returns the size in hand.
    /// `tests::an_animating_scene_asks_for_the_next_frame_only_while_it_animates`.
    fn picture(
        &mut self,
        renderer: Option<&mut GlesRenderer>,
        wanted: (i32, i32),
        scale: f64,
    ) -> Option<(i32, i32)> {
        let fresh = self.scene.needs_render();
        if self.buffers.current(wanted, fresh) {
            Some(wanted)
        } else {
            self.fill(renderer, wanted, scale)
        }
    }
}

/// Which plane a picture of the pointer goes on: the hardware cursor plane
/// (`Kind::Cursor`), unless it reads the backdrop, which the plane cannot
/// show, and then it is composited with everything else.
/// `tests::a_scene_with_no_material_is_a_cursor_plane_element`.
pub(crate) const fn plane(reads_backdrop: bool) -> Kind {
    if reads_backdrop {
        Kind::Unspecified
    } else {
        Kind::Cursor
    }
}

/// Where a picture's top-left corner goes, in physical pixels, for its
/// hotspot to sit on the pointer: the point every hit test asks about.
/// `tests::the_scene_is_drawn_at_its_own_size_with_its_hotspot_on_the_pointer`.
pub(crate) fn origin(
    location: Point<f64, Logical>,
    hotspot: Point<f64, Logical>,
    scale: f64,
) -> (f64, f64) {
    (
        (location.x - hotspot.x) * scale,
        (location.y - hotspot.y) * scale,
    )
}

/// One side of the scene, in whole logical pixels: what its root says,
/// rounded up, from one to [`LARGEST`].
fn side(logical: f64) -> i32 {
    if !logical.is_finite() {
        return 1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to LARGEST before the cast"
    )]
    let whole = logical.ceil().clamp(1.0, f64::from(LARGEST)) as i32;
    whole
}

/// A size in logical pixels as device pixels at `scale`, at least one each.
fn device(size: (i32, i32), scale: f64) -> (i32, i32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale.min(MAX_SCALE)
    } else {
        1.0
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "at most LARGEST times MAX_SCALE"
    )]
    let pixels = |logical: i32| ((f64::from(logical) * scale).round() as i32).max(1);
    (pixels(size.0), pixels(size.1))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use smithay::backend::renderer::element::Kind;
    use smithay::utils::{Logical, Point};

    use super::{Cursor, origin, plane};
    use crate::qml::pointer::tests::written;
    use crate::qml::qt_test::on_the_qt_thread;
    use crate::render::Drawn;

    /// The clock `qml::tick` is handed here: real time since this module's
    /// first use, so the animation runs at its own rate.
    fn now() -> Duration {
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        ORIGIN.get_or_init(Instant::now).elapsed()
    }

    /// One compositor frame over the scene, as `render::prepare` and
    /// `render::cursor` run it: the clock ticks, and the draw says whether the
    /// scene still has somewhere to go, which is what asks for the next frame.
    fn frame(cursor: &mut Cursor) -> bool {
        crate::qml::tick(now());
        Drawn::drawing(cursor, |cursor| {
            let size = cursor.own_size();
            cursor.picture(None, size, 1.0);
            None
        })
        .animating
    }

    /// **An animating scene asks for the next frame only while it animates**:
    /// a new scene asks once for its first picture and then nothing at rest; a
    /// 150 ms animation asks for every frame until it ends, and then for none,
    /// with nothing else on screen damaging anything.
    #[test]
    fn an_animating_scene_asks_for_the_next_frame_only_while_it_animates() {
        on_the_qt_thread(|| {
            let (directory, path) = written(
                "solium-pointer-scene-animates",
                r"
                import QtQuick
                Item {
                    id: root
                    width: 24; height: 24
                    property bool go: false
                    Rectangle {
                        width: 4; height: 4; color: 'red'
                        NumberAnimation on x { from: 0; to: 16; duration: 150; running: root.go }
                    }
                }
                ",
            );
            let mut cursor = Cursor::configured(&path, 24).expect("the scene builds");
            let first = frame(&mut cursor);
            let at_rest: Vec<bool> = (0..3).map(|_| frame(&mut cursor)).collect();
            cursor.scene.set_bool("go", true);
            let started = Instant::now();
            let mut running = Vec::new();
            while started.elapsed() < Duration::from_millis(400) {
                std::thread::sleep(Duration::from_millis(16));
                running.push((started.elapsed(), frame(&mut cursor)));
            }
            drop(cursor);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                (first, at_rest),
                (true, vec![false; 3]),
                "(the first picture, three frames at rest)"
            );
            let asked_while_running = running
                .iter()
                .filter(|(at, _)| *at < Duration::from_millis(120))
                .all(|(_, asked)| *asked);
            let asked_after = running
                .iter()
                .filter(|(at, _)| *at > Duration::from_millis(250))
                .any(|(_, asked)| *asked);
            assert!(
                asked_while_running && !asked_after,
                "a 150 ms animation asked for frames {running:?}"
            );
        });
    }

    /// **The scene is drawn at its own size, with its hotspot on the
    /// pointer**: a root of 40 by 32 logical pixels is larger than
    /// `cursor.size`, is drawn at 80 by 64 on a 2x monitor, and the pixel at
    /// its hotspot lands on the pointer's own point, which is where every hit
    /// test asks. A root that sets no size is `cursor.size` square.
    #[test]
    fn the_scene_is_drawn_at_its_own_size_with_its_hotspot_on_the_pointer() {
        on_the_qt_thread(|| {
            let (directory, path) = written(
                "solium-pointer-scene-hotspot",
                r##"
                import QtQuick
                import Solium
                Item {
                    width: 40; height: 32
                    Solium.cursor.hotspot: Qt.point(6, 4)
                    Rectangle { x: 6; y: 4; width: 1; height: 1; color: "#ff0000" }
                }
                "##,
            );
            let mut cursor = Cursor::configured(&path, 24).expect("the scene builds");
            let size = cursor.own_size();
            let held = cursor.picture(None, super::device(size, 2.0), 2.0);
            let pixel = |cursor: &mut Cursor, (x, y): (usize, usize)| {
                let mut found = [0_u8; 4];
                let buffer = cursor
                    .buffers
                    .get_mut((80, 64))
                    .expect("the picture is kept at its device size");
                let mut context = buffer.render();
                let _ = context.draw(|bytes| {
                    let at = (y * 80 + x) * 4;
                    found.copy_from_slice(&bytes[at..at + 4]);
                    Ok::<_, ()>(Vec::new())
                });
                found
            };
            // Premultiplied ARGB8888, little-endian: blue, green, red, alpha.
            let at_hotspot = pixel(&mut cursor, (12, 8));
            let beside = pixel(&mut cursor, (10, 6));
            let hotspot = cursor.hotspot();
            drop(cursor);
            std::fs::write(&path, "import QtQuick\nItem {}\n").expect("writing the scene");
            crate::qml::clear_cache();
            let bare = Cursor::configured(&path, 28)
                .expect("the scene builds")
                .own_size();
            let _ = std::fs::remove_dir_all(&directory);

            let location = Point::<f64, Logical>::from((100.0, 200.0));
            let corner = origin(location, hotspot, 2.0);
            assert_eq!(
                (size, held, at_hotspot, beside[3], bare),
                ((40, 32), Some((80, 64)), [0, 0, 255, 255], 0, (28, 28)),
                "(its own size, the picture at 2x, the pixel at its hotspot, the alpha beside \
                 it, a root with no size)"
            );
            assert_eq!(
                (corner.0 + 12.0, corner.1 + 8.0),
                (location.x * 2.0, location.y * 2.0),
                "the hotspot's pixel is not on the pointer: the picture's corner went to {corner:?}"
            );
        });
    }

    /// **A scene with no material is a cursor-plane element**, and so is one
    /// that sets a material, because materials are off until #199 and nothing
    /// reads the backdrop. The predicate itself puts a picture that reads the
    /// backdrop on the primary plane.
    #[test]
    fn a_scene_with_no_material_is_a_cursor_plane_element() {
        on_the_qt_thread(|| {
            let (directory, path) = written(
                "solium-pointer-scene-plane",
                "import QtQuick\nItem { width: 24; height: 24 }\n",
            );
            let plain = Cursor::configured(&path, 24)
                .expect("the scene builds")
                .kind();
            std::fs::write(
                &path,
                "import QtQuick\nimport Solium\nItem {\n    width: 24; height: 24\n    Rectangle { anchors.fill: parent; Solium.material: ({ effect: \"glass-rect\" }) }\n}\n",
            )
            .expect("writing the scene");
            crate::qml::clear_cache();
            let with_material = Cursor::configured(&path, 24)
                .expect("the scene builds")
                .kind();
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                [plain, with_material, plane(false), plane(true)],
                [Kind::Cursor, Kind::Cursor, Kind::Cursor, Kind::Unspecified],
                "[no material, a material while materials are off, nothing read, the backdrop \
                 read]"
            );
        });
    }
}
