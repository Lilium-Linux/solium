#![expect(
    unsafe_code,
    reason = "a beneath draw and a hole make their GL calls themselves, as the warp does"
)]

//! A backdrop drawn beneath a warped pane (Ruling F37, amended 2026-10-14,
//! live under motion and warps): a T2 result or a memo sampled at each
//! fragment's own position on the monitor, through the pane's warp mesh,
//! cut by the part's shape where the mesh's texture coordinates put it;
//! and the hole the pane's capture keeps in that shape. Generic: nothing
//! here names an effect. What it draws is wirecheck case 13i.

use smithay::{
    backend::renderer::{
        Texture as _,
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture, ffi},
        utils::{CommitCounter, OpaqueRegions},
    },
    utils::{Buffer, Logical, Physical, Rectangle, Scale, Size},
};

use crate::effect::{plan::Owner, rules::Slot};
use crate::warp::{BeneathProgram, Draw, Mesh};

/// A texture's uv from a position in an output's physical pixels:
/// `uv = pos * mul + add`. `tests::the_screen_map_reads_the_memo_where_each_fragment_is`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScreenMap {
    pub(crate) mul: [f32; 2],
    pub(crate) add: [f32; 2],
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "fractions of a monitor, given to GL"
)]
impl ScreenMap {
    /// A memo spans its monitor (Ruling F29): a position over the monitor's
    /// physical size, as `plan::place_on`'s `BoxMap` measures a box.
    /// `tests::the_screen_map_reads_the_memo_where_each_fragment_is`.
    pub(crate) fn monitor(monitor: Size<i32, Logical>, scale: f64) -> Self {
        let w = f64::from(monitor.w.max(1)) * scale;
        let h = f64::from(monitor.h.max(1)) * scale;
        Self {
            mul: [(1.0 / w) as f32, (1.0 / h) as f32],
            add: [0.0, 0.0],
        }
    }

    /// A T2 run's result spans its place's screen box (`T2Place.screen`).
    /// `tests::the_screen_map_reads_the_memo_where_each_fragment_is`.
    pub(crate) fn of_box(screen: Rectangle<i32, Physical>) -> Self {
        let w = f64::from(screen.size.w.max(1));
        let h = f64::from(screen.size.h.max(1));
        Self {
            mul: [(1.0 / w) as f32, (1.0 / h) as f32],
            add: [
                (-f64::from(screen.loc.x) / w) as f32,
                (-f64::from(screen.loc.y) / h) as f32,
            ],
        }
    }

    /// `tests::the_screen_map_reads_the_memo_where_each_fragment_is`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the shader does this sum; the tests ask it")
    )]
    pub(crate) fn at(&self, pos: (f32, f32)) -> (f32, f32) {
        (
            pos.0 * self.mul[0] + self.add[0],
            pos.1 * self.mul[1] + self.add[1],
        )
    }
}

/// A part's shape in the warped capture's own pixels, as
/// `fragment::MASKED_TEXTURE` measures a mask: its rect, its corner radii
/// (top-left, top-right, bottom-left, bottom-right), the capture's size.
/// `tests::a_beneath_mask_is_the_part_in_the_captures_pixels`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shape {
    pub(crate) rect: [f32; 4],
    pub(crate) radii: [f32; 4],
    pub(crate) size: [f32; 2],
}

/// What cuts a beneath draw: the part's shape (Task 47 adds its alpha).
#[derive(Clone, Debug)]
pub(crate) enum BeneathMask {
    Shape(Shape),
}

/// What a beneath texture spans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Over {
    /// A memo, the whole monitor.
    Monitor,
    /// A T2 run's result, its place's box on the monitor.
    Box(Rectangle<i32, Physical>),
}

/// A beneath place's texture this pass, as `render::run_t2` left it: what
/// the warp's `Under` on that monitor draws, and whether it re-ran, which
/// moves its commit
/// (`effect::store::tests::an_unders_commit_moves_with_its_warp_or_its_rerun`).
#[derive(Clone, Debug)]
pub(crate) struct Screened {
    pub(crate) texture: GlesTexture,
    pub(crate) over: Over,
    pub(crate) rerun: bool,
}

/// One slot drawn beneath a warped pane on one monitor: made in the
/// `warps` phase (`render::unders`), drawn by `elements` through that
/// monitor's mesh as a [`Beneath`].
#[derive(Clone, Debug)]
pub(crate) struct Under {
    pub(crate) monitor: String,
    pub(crate) texture: GlesTexture,
    pub(crate) over: Over,
    pub(crate) mask: BeneathMask,
    pub(crate) id: Id,
    pub(crate) commit: CommitCounter,
    /// The slot's opacity this pass, 1 but while its rule's fade moves it
    /// (`effect::fade`), over the warp's own alpha.
    pub(crate) opacity: f32,
}

/// A part's rest rectangle in its pane's capture, at the capture's scale,
/// `outer` the rectangle the capture holds: the pane grown by its layers'
/// bleed (`render::bled`).
/// `tests::a_beneath_mask_is_the_part_in_the_captures_pixels`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "capture pixels, given to GL"
)]
pub(crate) fn beneath_mask(
    part: Rectangle<i32, Logical>,
    outer: Rectangle<i32, Logical>,
    radii: [f32; 4],
    scale: f64,
) -> Shape {
    let at = |n: i32| (f64::from(n) * scale) as f32;
    Shape {
        rect: [
            at(part.loc.x - outer.loc.x),
            at(part.loc.y - outer.loc.y),
            at(part.size.w),
            at(part.size.h),
        ],
        radii,
        size: [at(outer.size.w), at(outer.size.h)],
    }
}

/// A slot's backdrop beneath a warped pane: its texture sampled where each
/// fragment of the pane's mesh is on the monitor, cut by the part's shape
/// at the mesh's texture coordinates. Drawn just after (so beneath) the
/// pane's warp in `render::panes`. Its commit moves with the warp's or a
/// re-run (`Under::commit`); its bounds are the mesh's.
#[derive(Clone, Debug)]
pub(crate) struct Beneath {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    mesh: Mesh,
    map: ScreenMap,
    mask: Shape,
    alpha: f32,
    program: BeneathProgram,
    bounds: Rectangle<i32, Physical>,
}

impl Beneath {
    pub(crate) fn new(
        under: &Under,
        mesh: Mesh,
        map: ScreenMap,
        alpha: f32,
        program: BeneathProgram,
    ) -> Self {
        let BeneathMask::Shape(mask) = &under.mask;
        Self {
            id: under.id.clone(),
            commit: under.commit,
            texture: under.texture.clone(),
            bounds: mesh.bounds(),
            mesh,
            map,
            mask: *mask,
            alpha: alpha * under.opacity,
            program,
        }
    }
}

/// The hole a warped pane's capture keeps where a slot drawn beneath the
/// warp lies: the part's shape, blended `ZERO, ONE_MINUS_SRC_ALPHA`, so
/// what the pane's lower layers drew there is taken out, as the slot's
/// opaque result covered it on the flat path. Its id is the slot's
/// (`SlotState.hole`) and its commit never moves: its geometry keys the
/// capture (`keyed::Inputs::of`), and a radius changed alone redraws the
/// decoration, which redraws the capture.
/// `render::tests::a_beneath_slot_is_a_hole_only_inside_a_capture`.
#[derive(Clone, Debug)]
pub(crate) struct Hole {
    owner: Owner,
    slot: Slot,
    id: Id,
    bounds: Rectangle<i32, Physical>,
    mesh: Mesh,
    mask: Shape,
    program: BeneathProgram,
}

impl Hole {
    pub(crate) fn new(
        owner: Owner,
        slot: Slot,
        id: Id,
        rect: Rectangle<f64, Physical>,
        radii: [f32; 4],
        program: BeneathProgram,
    ) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "capture pixels, given to GL"
        )]
        let (w, h) = (rect.size.w as f32, rect.size.h as f32);
        let mesh = Mesh::quad(rect);
        Self {
            owner,
            slot,
            id,
            bounds: mesh.bounds(),
            mesh,
            mask: Shape {
                rect: [0.0, 0.0, w, h],
                radii,
                size: [w, h],
            },
            program,
        }
    }

    /// The slot it stands for, which orders its `Under` (`render::unders`).
    pub(crate) fn owner(&self) -> (&Owner, Slot) {
        (&self.owner, self.slot)
    }
}

impl Element for Beneath {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(self.texture.size().to_f64())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.bounds
    }

    /// None: what is beneath a warp is antialiased at its edges, as the
    /// warp's own texture is (`warp::Warp`).
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Beneath {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let draw = Draw::Backdrop {
            map: [
                self.map.mul[0],
                self.map.mul[1],
                self.map.add[0],
                self.map.add[1],
            ],
            alpha: self.alpha,
        };
        through(
            frame,
            (dst, damage),
            self.program,
            self.texture.tex_id(),
            &self.mesh,
            draw,
            self.mask,
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // Never a plane: it is not a rectangle.
        None
    }
}

impl Element for Hole {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        CommitCounter::default()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size((f64::from(self.mask.size[0]), f64::from(self.mask.size[1])).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.bounds
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Hole {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        through(
            frame,
            (dst, damage),
            self.program,
            0,
            &self.mesh,
            Draw::Erase,
            self.mask,
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// Draw `mesh` through `program` under the damage, each rectangle under its
/// own scissor, as `warp::Warp` draws (wirecheck case 11g's reason: a mesh
/// drawn whole under partial damage blends its edge twice).
fn through(
    frame: &mut GlesFrame<'_, '_>,
    (dst, damage): (Rectangle<i32, Physical>, &[Rectangle<i32, Physical>]),
    program: BeneathProgram,
    texture: ffi::types::GLuint,
    mesh: &Mesh,
    draw: Draw,
    mask: Shape,
) -> Result<(), GlesError> {
    if damage.is_empty() {
        return Ok(());
    }
    let projection = *frame.projection();
    let vertices = mesh.interleaved();
    let rects: Vec<Rectangle<i32, Physical>> = damage
        .iter()
        .map(|rect| Rectangle::new(dst.loc + rect.loc, rect.size))
        .collect();
    frame.with_context(|gl| {
        let mut viewport = [0_i32; 4];
        // SAFETY: a context is current inside `with_context`; the program's
        // names, the texture and the buffer were made against this
        // renderer's context, between frames (`pass::Programs::beneath`).
        unsafe {
            gl.GetIntegerv(ffi::VIEWPORT, viewport.as_mut_ptr());
            let scissors: Vec<[i32; 4]> = rects
                .iter()
                .map(|rect| crate::warp::scissor_box(&projection, viewport, *rect))
                .collect();
            program.draw(
                gl,
                &projection,
                texture,
                &vertices,
                draw,
                (mask.rect, mask.radii, mask.size),
                &scissors,
            );
        }
    })
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Logical, Rectangle};

    use super::{ScreenMap, Shape, beneath_mask};

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5
    }

    /// **The screen map reads the memo where each fragment is**: on a
    /// 1920×1080 monitor at scale 1.25, a fragment at a place's corner
    /// reads the memo at the place's `BoxMap` offset, and one at its far
    /// corner at offset plus scale, which is `plan::place_on`'s arithmetic,
    /// the one a crop reads; a T2 result's map spans its own screen box.
    #[test]
    fn the_screen_map_reads_the_memo_where_each_fragment_is() {
        let monitor = Rectangle::<i32, Logical>::new((1920, 0).into(), (1920, 1080).into());
        let padded = Rectangle::<f64, Logical>::new((2120.0, 200.0).into(), (400.0, 240.0).into());
        let (_, map, _) = crate::effect::plan::place_on(padded, monitor, 1.25, (2400, 1350).into());
        let memo = ScreenMap::monitor(monitor.size, 1.25);
        assert!(close(
            memo.at((250.0, 250.0)),
            (map.offset[0], map.offset[1])
        ));
        assert!(close(
            memo.at((750.0, 550.0)),
            (map.offset[0] + map.scale[0], map.offset[1] + map.scale[1])
        ));
        let result = ScreenMap::of_box(Rectangle::new((250, 250).into(), (500, 300).into()));
        assert!(close(result.at((250.0, 250.0)), (0.0, 0.0)));
        assert!(close(result.at((750.0, 550.0)), (1.0, 1.0)));
    }

    /// **A beneath mask is the part in the capture's pixels**: a client
    /// 28 px under a 400×300 pane's top, at scale 1.5, is (0, 42, 600, 408)
    /// of a 600×450 capture, its radii as given.
    #[test]
    fn a_beneath_mask_is_the_part_in_the_captures_pixels() {
        let outer = Rectangle::<i32, Logical>::new((1000, 100).into(), (400, 300).into());
        let client = Rectangle::<i32, Logical>::new((1000, 128).into(), (400, 272).into());
        assert_eq!(
            beneath_mask(client, outer, [0.0, 0.0, 12.0, 12.0], 1.5),
            Shape {
                rect: [0.0, 42.0, 600.0, 408.0],
                radii: [0.0, 0.0, 12.0, 12.0],
                size: [600.0, 450.0]
            }
        );
    }
}
