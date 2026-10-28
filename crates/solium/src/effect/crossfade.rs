#![expect(
    unsafe_code,
    reason = "a crossfade binds its other pictures on texture units of their own, which smithay's texture draw leaves alone"
)]

//! A slot crossfading (#389, `effect::fade`), drawn as one element: up to
//! three pictures of the slot, read in one draw through
//! `fragment::CROSSFADE_TEXTURE`, each pixel their sum at their weights, and
//! that laid over what is under the slot once. The pictures are two rules'
//! results as the slot hands over from one rule to the other, and for a
//! `replace` the part's own pixels as the slot hands over to or from no rule
//! at all.
//!
//! Why one draw: pictures laid one over another at opposite weights are not
//! their mix. The upper one covers the lower as well as adding to it, so a
//! window's own pixels at 0.95 and at 0.80, crossfaded that way, would pass
//! through 0.69 half way, below both, a dip on every change of focus; and a
//! `replace` fading its result out over nothing shows what is behind the
//! window through it until the part is drawn again, at once, at the end.
//! Mixed first, they pass through 0.875 there, through every opacity between
//! the two, and never outside them; and the mix is drawn in the part's place,
//! so nothing of the part is drawn raw under it.
//!
//! Smithay and `solium_effects` only, beside `element`, so `dev/wirecheck`
//! includes this file by `#[path]` and its case 12k draws it on the GPU.

use smithay::backend::renderer::{
    Texture as _,
    element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
    gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformValue, ffi},
    utils::{CommitCounter, OpaqueRegions},
};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};
use solium_effects::fragment::{
    CROSSFADE_CUT_UNIFORM, CROSSFADE_FIRST_BOUNDS_UNIFORM, CROSSFADE_SECOND_BOUNDS_UNIFORM,
    CROSSFADE_SECOND_MAP_UNIFORM, CROSSFADE_SECOND_UNIFORM, CROSSFADE_THIRD_BOUNDS_UNIFORM,
    CROSSFADE_THIRD_MAP_UNIFORM, CROSSFADE_THIRD_UNIFORM, CROSSFADE_WEIGHTS_UNIFORM,
    MASK_RADII_UNIFORM, MASK_RECT_UNIFORM, MASK_SIZE_UNIFORM,
};

use super::element::{EffectElement, MaskUniforms, Placement, mask_uniforms, src_of};

/// The box a slot's pictures are drawn over together, and what the program
/// is given to read each where it is placed: the first through smithay's
/// own texture coordinate over `src`, the others at that coordinate through
/// their `maps`, each nothing past its own `bounds`, and the part's mask,
/// cutting each as `cut` says. A picture not given reads nothing. `tests`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mixed {
    /// Every picture's box together, on the output.
    pub(crate) dst: Rectangle<i32, Physical>,
    /// What of the first's texture spans `dst`, in its pixels: past the
    /// texture, where another's box reaches beyond the first's.
    pub(crate) src: Rectangle<f64, Buffer>,
    /// The second's and third's texture coordinates from the first's,
    /// `(mul.x, mul.y, add.x, add.y)`.
    pub(crate) maps: [[f32; 4]; 2],
    /// Each picture in its texture's coordinates, `(x0, y0, x1, y1)`.
    pub(crate) bounds: [[f32; 4]; 3],
    /// Whether the mask cuts each, 1 or 0.
    pub(crate) cut: [f32; 3],
    /// The part's mask, measured over `dst` as `element::mask_uniforms`
    /// measures one over a placement.
    pub(crate) mask: MaskUniforms,
}

/// One picture's texture coordinate from a position on the output, per axis
/// `x * mul + add`; where it lies in those coordinates; and how many of its
/// texture's pixels one of the output's is.
struct Reading {
    mul: [f64; 2],
    add: [f64; 2],
    bounds: [f64; 4],
    per_pixel: [f64; 2],
}

fn reading(placement: Placement, texture: (i32, i32)) -> Reading {
    let src = src_of(placement, texture);
    let (w, h) = (f64::from(texture.0.max(1)), f64::from(texture.1.max(1)));
    let dst = placement.dst;
    let per_pixel = [
        src.size.w / f64::from(dst.size.w.max(1)),
        src.size.h / f64::from(dst.size.h.max(1)),
    ];
    Reading {
        mul: [per_pixel[0] / w, per_pixel[1] / h],
        add: [
            (src.loc.x - f64::from(dst.loc.x) * per_pixel[0]) / w,
            (src.loc.y - f64::from(dst.loc.y) * per_pixel[1]) / h,
        ],
        bounds: [
            src.loc.x / w,
            src.loc.y / h,
            (src.loc.x + src.size.w) / w,
            (src.loc.y + src.size.h) / h,
        ],
        per_pixel,
    }
}

/// Where a slot's pictures, each placed as the walk placed it with its
/// texture's size, are drawn mixed: over all their boxes, each read where
/// its own placement puts it. The part's mask is the one any carries (all
/// are the slot's part's), and cuts each that carries it. At most two more
/// than the first are read.
/// `tests::two_results_in_one_box_read_each_whole`,
/// `tests::a_result_reaching_further_widens_the_box_and_reads_the_other_where_it_lies`,
/// `tests::a_crop_is_its_results_bounds`,
/// `tests::a_third_picture_is_read_where_it_lies`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "texture coordinates and pixels, as the f32 the uniforms take"
)]
pub(crate) fn mixed(first: (Placement, (i32, i32)), more: &[(Placement, (i32, i32))]) -> Mixed {
    let more = &more[..more.len().min(2)];
    let dst = more
        .iter()
        .fold(first.0.dst, |dst, (placement, _)| dst.merge(placement.dst));
    let a = reading(first.0, first.1);
    let src_a = src_of(first.0, first.1);
    let src = Rectangle::<f64, Buffer>::new(
        (
            src_a.loc.x + f64::from(dst.loc.x - first.0.dst.loc.x) * a.per_pixel[0],
            src_a.loc.y + f64::from(dst.loc.y - first.0.dst.loc.y) * a.per_pixel[1],
        )
            .into(),
        (
            f64::from(dst.size.w) * a.per_pixel[0],
            f64::from(dst.size.h) * a.per_pixel[1],
        )
            .into(),
    );
    let four = |v: [f64; 4]| [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32];
    // A position is `(uv_first - a.add) / a.mul`, and another's coordinate
    // there `position * b.mul + b.add`.
    let map = |b: &Reading| {
        let ratio = |axis: usize| {
            if a.mul[axis].abs() < f64::EPSILON {
                0.0
            } else {
                b.mul[axis] / a.mul[axis]
            }
        };
        let mul = [ratio(0), ratio(1)];
        four([
            mul[0],
            mul[1],
            b.add[0] - a.add[0] * mul[0],
            b.add[1] - a.add[1] * mul[1],
        ])
    };
    let mut maps = [[0.0; 4]; 2];
    let mut bounds = [four(a.bounds), [0.0; 4], [0.0; 4]];
    let cut_of = |placement: Placement| if placement.mask.is_some() { 1.0 } else { 0.0 };
    let mut cut = [cut_of(first.0), 0.0, 0.0];
    for (index, (placement, texture)) in more.iter().enumerate() {
        let b = reading(*placement, *texture);
        maps[index] = map(&b);
        bounds[index + 1] = four(b.bounds);
        cut[index + 1] = cut_of(*placement);
    }
    let shape = std::iter::once(&first)
        .chain(more)
        .find_map(|(placement, _)| placement.mask.map(|mask| (mask, placement.dst)));
    let mask = shape.map(|((rect, radii), at)| {
        (
            Rectangle::<f64, Physical>::new(
                (
                    rect.loc.x + f64::from(at.loc.x - dst.loc.x),
                    rect.loc.y + f64::from(at.loc.y - dst.loc.y),
                )
                    .into(),
                rect.size,
            ),
            radii,
        )
    });
    let measured = mask_uniforms(
        Placement {
            dst,
            mask,
            alpha: first.0.alpha,
            crop: Some(src),
        },
        first.1,
    )
    .unwrap_or(MaskUniforms {
        rect: [0.0; 4],
        radii: [0.0; 4],
        size: [1.0, 1.0],
    });
    Mixed {
        dst,
        src,
        maps,
        bounds,
        cut,
        mask: measured,
    }
}

/// A slot's pictures drawn mixed at their weights, through the crossfade
/// program: what the walk draws in a slot crossfading.
#[derive(Clone, Debug)]
pub(crate) struct CrossfadeElement {
    id: Id,
    alpha: f32,
    pictures: Vec<EffectElement>,
    weights: [f32; 3],
    program: GlesTexProgram,
    mixed: Mixed,
    commit: CommitCounter,
}

impl CrossfadeElement {
    /// `pictures`, one to three, each placed as the walk places a slot's
    /// result at the slot's alpha (the first's), with its weight; the
    /// weights sum to 1. It takes the first's id, the slot's own result's,
    /// so the slot is one element to the damage tracker through its
    /// crossfade and after; its commit moves with any picture's, the
    /// weights and the box. `None` with no picture.
    pub(crate) fn new(pictures: &[(EffectElement, f32)], program: GlesTexProgram) -> Option<Self> {
        let pictures = &pictures[..pictures.len().min(3)];
        let ((first, _), more) = pictures.split_first()?;
        let size = |element: &EffectElement| {
            let size = element.texture().size();
            (size.w, size.h)
        };
        let placed: Vec<(Placement, (i32, i32))> = more
            .iter()
            .map(|(element, _)| (element.placement(), size(element)))
            .collect();
        let mixed = mixed((first.placement(), size(first)), &placed);
        let mut weights = [0.0; 3];
        for (weight, (_, given)) in weights.iter_mut().zip(pictures) {
            *weight = *given;
        }
        let pictures: Vec<EffectElement> = pictures
            .iter()
            .map(|(element, _)| element.clone())
            .collect();
        let commit = commit_of(&pictures, weights, &mixed);
        Some(Self {
            id: first.id().clone(),
            alpha: first.placement().alpha,
            pictures,
            weights,
            program,
            mixed,
            commit,
        })
    }
}

/// A commit that moves whenever what the crossfade draws may have: any
/// picture's commit, the weights, the box, or where the others are read.
fn commit_of(pictures: &[EffectElement], weights: [f32; 3], mixed: &Mixed) -> CommitCounter {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for picture in pictures {
        picture
            .current_commit()
            .distance(Some(CommitCounter::default()))
            .hash(&mut hasher);
        picture.placement().alpha.to_bits().hash(&mut hasher);
    }
    for value in weights.iter().chain(mixed.maps.iter().flatten()) {
        value.to_bits().hash(&mut hasher);
    }
    (
        mixed.dst.loc.x,
        mixed.dst.loc.y,
        mixed.dst.size.w,
        mixed.dst.size.h,
    )
        .hash(&mut hasher);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a hash, of which any bits will do"
    )]
    let commit = hasher.finish() as usize;
    CommitCounter::from(commit)
}

impl Element for CrossfadeElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.mixed.src
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.mixed.dst
    }

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

impl RenderElement<GlesRenderer> for CrossfadeElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let Some(first) = self.pictures.first() else {
            return Ok(());
        };
        // A picture not given is the first again, at weight 0.
        let name = |index: usize| self.pictures.get(index).unwrap_or(first).texture().tex_id();
        let names = [name(0), name(1), name(2)];
        // SAFETY: inside the frame's own context, every texture this
        // renderer's; smithay binds the first on unit 0 itself, and units 1
        // and 2 are given back below.
        frame.with_context(|gl| unsafe { bind(gl, names) })?;
        let mixed = self.mixed;
        let drawn = frame.render_texture_from_to(
            first.texture(),
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            Some(&self.program),
            &[
                Uniform::new(CROSSFADE_SECOND_UNIFORM, UniformValue::_1i(1)),
                Uniform::new(CROSSFADE_THIRD_UNIFORM, UniformValue::_1i(2)),
                Uniform::new(CROSSFADE_WEIGHTS_UNIFORM, self.weights),
                Uniform::new(CROSSFADE_SECOND_MAP_UNIFORM, mixed.maps[0]),
                Uniform::new(CROSSFADE_THIRD_MAP_UNIFORM, mixed.maps[1]),
                Uniform::new(CROSSFADE_FIRST_BOUNDS_UNIFORM, mixed.bounds[0]),
                Uniform::new(CROSSFADE_SECOND_BOUNDS_UNIFORM, mixed.bounds[1]),
                Uniform::new(CROSSFADE_THIRD_BOUNDS_UNIFORM, mixed.bounds[2]),
                Uniform::new(CROSSFADE_CUT_UNIFORM, mixed.cut),
                Uniform::new(MASK_RECT_UNIFORM, mixed.mask.rect),
                Uniform::new(MASK_RADII_UNIFORM, mixed.mask.radii),
                Uniform::new(MASK_SIZE_UNIFORM, mixed.mask.size),
            ],
        );
        // SAFETY: as above.
        frame.with_context(|gl| unsafe { unbind(gl) })?;
        drawn
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// The second and third pictures on units 1 and 2, and every picture read
/// linear and clamped at its edges, which a texture made by `create_buffer`
/// is not until told (`run.rs`); unit 0 left active and empty, as smithay
/// binds the first itself.
///
/// # Safety
/// Inside a current context whose names `names` are.
unsafe fn bind(gl: &ffi::Gles2, names: [ffi::types::GLuint; 3]) {
    let linear = i32::try_from(ffi::LINEAR).unwrap_or_default();
    let edge = i32::try_from(ffi::CLAMP_TO_EDGE).unwrap_or_default();
    // SAFETY: the caller's contract.
    unsafe {
        for (unit, name) in [ffi::TEXTURE0, ffi::TEXTURE1, ffi::TEXTURE2]
            .into_iter()
            .zip(names)
        {
            gl.ActiveTexture(unit);
            gl.BindTexture(ffi::TEXTURE_2D, name);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, linear);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, linear);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, edge);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, edge);
        }
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, 0);
    }
}

/// Units 1 and 2 empty again and unit 0 active: what smithay expects to
/// find.
///
/// # Safety
/// Inside a current context.
unsafe fn unbind(gl: &ffi::Gles2) {
    // SAFETY: the caller's contract.
    unsafe {
        for unit in [ffi::TEXTURE2, ffi::TEXTURE1] {
            gl.ActiveTexture(unit);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
        }
        gl.ActiveTexture(ffi::TEXTURE0);
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Buffer, Physical, Rectangle};
    use solium_effects::fragment::Corners;

    use super::mixed;
    use crate::effect::element::Placement;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// `to`'s texture coordinate at `from`'s, as the program maps it.
    fn to_at(map: [f32; 4], uv: (f32, f32)) -> (f32, f32) {
        (uv.0 * map[0] + map[2], uv.1 * map[1] + map[3])
    }

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5
    }

    /// **Two results in one box read each whole**: a window's own pixels
    /// at two opacities, each a 200×100 result placed over the same box,
    /// are drawn over that box, each read where the other is, every texel
    /// of each its own, the mask cutting both.
    #[test]
    fn two_results_in_one_box_read_each_whole() {
        let mask = Some((
            Rectangle::<f64, Physical>::new((0.0, 0.0).into(), (200.0, 100.0).into()),
            Corners::all(12.0),
        ));
        let placed = Placement::of(rect(40, 30, 200, 100), mask, 0.5);
        let got = mixed((placed, (200, 100)), &[(placed, (200, 100))]);
        assert_eq!(got.dst, rect(40, 30, 200, 100));
        assert_eq!(
            got.src,
            Rectangle::<f64, Buffer>::new((0.0, 0.0).into(), (200.0, 100.0).into())
        );
        assert_eq!(got.maps[0], [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(got.bounds[0], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(got.bounds[1], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(got.cut, [1.0, 1.0, 0.0]);
        assert_eq!(got.mask.rect, [0.0, 0.0, 200.0, 100.0]);
        assert_eq!(got.mask.size, [200.0, 100.0]);
        assert_eq!(got.mask.radii, [12.0; 4]);
    }

    /// **A result reaching further widens the box and reads the other where
    /// it lies**: `to` padded by a reach of 10 around the same 100×50 part
    /// as `from`, and not cut. The box is `to`'s; `from` is read past its
    /// texture there, which its bounds make nothing, and at its own corner
    /// `to` is read 10 pixels into its padding; the mask, `from`'s, is the
    /// part's, 10 pixels inside the box's corner, and cuts `from` alone.
    #[test]
    fn a_result_reaching_further_widens_the_box_and_reads_the_other_where_it_lies() {
        let part = Some((
            Rectangle::<f64, Physical>::new((0.0, 0.0).into(), (100.0, 50.0).into()),
            Corners::all(0.0),
        ));
        let from = Placement::of(rect(100, 100, 100, 50), part, 1.0);
        let to = Placement::of(rect(90, 90, 120, 70), None, 1.0);
        let got = mixed((from, (100, 50)), &[(to, (120, 70))]);
        assert_eq!(got.dst, rect(90, 90, 120, 70));
        assert_eq!(
            got.src,
            Rectangle::<f64, Buffer>::new((-10.0, -10.0).into(), (120.0, 70.0).into())
        );
        assert!(close(
            to_at(got.maps[0], (0.0, 0.0)),
            (10.0 / 120.0, 10.0 / 70.0)
        ));
        assert!(close(
            to_at(got.maps[0], (1.0, 1.0)),
            (110.0 / 120.0, 60.0 / 70.0)
        ));
        assert_eq!(got.bounds[0], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(got.cut, [1.0, 0.0, 0.0]);
        // The program's pixel in the mask, `v * size - rect.xy`, is 0 at
        // `from`'s corner (v = 0), the part's, and -10 at the box's, 10
        // pixels out of the part.
        assert_eq!(got.mask.rect, [0.0, 0.0, 100.0, 50.0]);
        let v_at_box_corner = (-10.0_f32 / 100.0, -10.0_f32 / 50.0);
        let px = (
            v_at_box_corner.0 * got.mask.size[0] - got.mask.rect[0],
            v_at_box_corner.1 * got.mask.size[1] - got.mask.rect[1],
        );
        assert!(close(px, (-10.0, -10.0)), "{px:?}");
    }

    /// **A crop is its result's bounds**: a memo's top-right quarter,
    /// cropped and placed, is read only there; past it, the rest of the memo
    /// is not the result's.
    #[test]
    fn a_crop_is_its_results_bounds() {
        let crop = Rectangle::<f64, Buffer>::new((148.0, 0.0).into(), (148.0, 98.0).into());
        let from = Placement::of(rect(0, 0, 148, 98), None, 1.0).cropped(crop);
        let to = Placement::of(rect(0, 0, 148, 98), None, 1.0);
        let got = mixed((from, (296, 196)), &[(to, (148, 98))]);
        assert_eq!(got.bounds[0], [0.5, 0.0, 1.0, 0.5]);
        assert!(close(to_at(got.maps[0], (0.5, 0.0)), (0.0, 0.0)));
        assert!(close(to_at(got.maps[0], (1.0, 0.5)), (1.0, 1.0)));
        assert_eq!(got.cut, [0.0, 0.0, 0.0]);
    }

    /// **A third picture is read where it lies**: a `replace` crossfading
    /// between two rules as its part takes the slot back, the part's own
    /// pixels a capture padded by 4 around the 100×50 part the two results
    /// lie exactly over; and a fourth is not read.
    #[test]
    fn a_third_picture_is_read_where_it_lies() {
        let result = Placement::of(rect(100, 100, 100, 50), None, 1.0);
        let raw = Placement::of(rect(96, 96, 108, 58), None, 1.0);
        let got = mixed(
            (result, (100, 50)),
            &[(result, (100, 50)), (raw, (108, 58)), (raw, (108, 58))],
        );
        assert_eq!(got.dst, rect(96, 96, 108, 58));
        assert_eq!(got.maps[0], [1.0, 1.0, 0.0, 0.0]);
        assert!(close(
            to_at(got.maps[1], (0.0, 0.0)),
            (4.0 / 108.0, 4.0 / 58.0)
        ));
        assert_eq!(got.bounds[2], [0.0, 0.0, 1.0, 1.0]);
    }
}
