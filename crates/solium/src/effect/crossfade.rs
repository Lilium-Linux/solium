#![expect(
    unsafe_code,
    reason = "a crossfade binds the result it crossfades to on a texture unit of its own, which smithay's texture draw leaves alone"
)]

//! A slot crossfading from one rule's result to another's (#389,
//! `effect::fade`), drawn as one element: both results read in one draw
//! through `fragment::CROSSFADE_TEXTURE`, each pixel the two mixed at the
//! crossfade's share, and that laid over what is under the slot once.
//!
//! Why one draw: two results laid one over the other at opposite weights are
//! not their mix. The upper one covers the lower as well as adding to it, so
//! a window's own pixels at 0.95 and at 0.80, crossfaded that way, would pass
//! through 0.69 half way, below both, a dip on every change of focus. Mixed
//! first, they pass through 0.875 there, through every opacity between the
//! two, and never outside them; and a `replace` slot draws the mix in the
//! part's place, so nothing of the part is drawn raw under it.
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
    CROSSFADE_CUT_UNIFORM, CROSSFADE_FROM_BOUNDS_UNIFORM, CROSSFADE_SHARE_UNIFORM,
    CROSSFADE_TO_BOUNDS_UNIFORM, CROSSFADE_TO_MAP_UNIFORM, CROSSFADE_TO_UNIFORM,
    MASK_RADII_UNIFORM, MASK_RECT_UNIFORM, MASK_SIZE_UNIFORM,
};

use super::element::{EffectElement, MaskUniforms, Placement, mask_uniforms, src_of};

/// The box two results are drawn over together, and what the program is
/// given to read each where it is placed: `from` through smithay's own
/// texture coordinate over `src`, `to` at that coordinate through `to_map`,
/// each nothing past its own `bounds`, and the part's mask, cutting each as
/// `cut` says. `tests`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mixed {
    /// Both placements' boxes together, on the output.
    pub(crate) dst: Rectangle<i32, Physical>,
    /// What of `from`'s texture spans `dst`, in its pixels: past the
    /// texture, where `to`'s box reaches beyond `from`'s.
    pub(crate) src: Rectangle<f64, Buffer>,
    /// `to`'s texture coordinate from `from`'s, `(mul.x, mul.y, add.x, add.y)`.
    pub(crate) to_map: [f32; 4],
    /// Each result in its texture's coordinates, `(x0, y0, x1, y1)`.
    pub(crate) from_bounds: [f32; 4],
    pub(crate) to_bounds: [f32; 4],
    /// Whether the mask cuts `from` and `to`, 1 or 0.
    pub(crate) cut: [f32; 2],
    /// The part's mask, measured over `dst` as `element::mask_uniforms`
    /// measures one over a placement.
    pub(crate) mask: MaskUniforms,
}

/// One result's texture coordinate from a position on the output, per axis
/// `x * mul + add`; where its result lies in those coordinates; and how many
/// of its texture's pixels one of the output's is.
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

/// Where two results of one slot, each placed as the walk placed it with
/// its texture's size, are drawn mixed: over both their boxes, each read
/// where its own placement puts it. The part's mask is the one either
/// carries (both are the slot's part's), and cuts each that carries it.
/// `tests::two_results_in_one_box_read_each_whole`,
/// `tests::a_result_reaching_further_widens_the_box_and_reads_the_other_where_it_lies`,
/// `tests::a_crop_is_its_results_bounds`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "texture coordinates and pixels, as the f32 the uniforms take"
)]
pub(crate) fn mixed(from: (Placement, (i32, i32)), to: (Placement, (i32, i32))) -> Mixed {
    let dst = from.0.dst.merge(to.0.dst);
    let (a, b) = (reading(from.0, from.1), reading(to.0, to.1));
    let src_a = src_of(from.0, from.1);
    let src = Rectangle::<f64, Buffer>::new(
        (
            src_a.loc.x + f64::from(dst.loc.x - from.0.dst.loc.x) * a.per_pixel[0],
            src_a.loc.y + f64::from(dst.loc.y - from.0.dst.loc.y) * a.per_pixel[1],
        )
            .into(),
        (
            f64::from(dst.size.w) * a.per_pixel[0],
            f64::from(dst.size.h) * a.per_pixel[1],
        )
            .into(),
    );
    // A position is `(uv_from - a.add) / a.mul`, and `to`'s coordinate there
    // `position * b.mul + b.add`.
    let ratio = |axis: usize| {
        if a.mul[axis].abs() < f64::EPSILON {
            0.0
        } else {
            b.mul[axis] / a.mul[axis]
        }
    };
    let mul = [ratio(0), ratio(1)];
    let add = [b.add[0] - a.add[0] * mul[0], b.add[1] - a.add[1] * mul[1]];
    let shape = from
        .0
        .mask
        .map(|mask| (mask, from.0.dst))
        .or_else(|| to.0.mask.map(|mask| (mask, to.0.dst)));
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
            alpha: from.0.alpha,
            crop: Some(src),
        },
        from.1,
    )
    .unwrap_or(MaskUniforms {
        rect: [0.0; 4],
        radii: [0.0; 4],
        size: [1.0, 1.0],
    });
    let four = |v: [f64; 4]| [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32];
    let cut = |placement: Placement| if placement.mask.is_some() { 1.0 } else { 0.0 };
    Mixed {
        dst,
        src,
        to_map: four([mul[0], mul[1], add[0], add[1]]),
        from_bounds: four(a.bounds),
        to_bounds: four(b.bounds),
        cut: [cut(from.0), cut(to.0)],
        mask: measured,
    }
}

/// Two results of one slot drawn mixed at `share`, through the crossfade
/// program: what the walk draws in a slot crossfading between two rules.
#[derive(Clone, Debug)]
pub(crate) struct CrossfadeElement {
    from: EffectElement,
    to: EffectElement,
    share: f32,
    program: GlesTexProgram,
    mixed: Mixed,
    commit: CommitCounter,
}

impl CrossfadeElement {
    /// `from` and `to`, each placed as the walk places a slot's result, at
    /// the slot's alpha, mixed at `share` (0 all `from`, 1 all `to`). It
    /// takes `to`'s id, the slot's result's, so the slot is one element to
    /// the damage tracker through its crossfade and after; its commit moves
    /// with either result's, the share and the box.
    pub(crate) fn new(
        from: EffectElement,
        to: EffectElement,
        share: f32,
        program: GlesTexProgram,
    ) -> Self {
        let size = |element: &EffectElement| {
            let size = element.texture().size();
            (size.w, size.h)
        };
        let mixed = mixed((from.placement(), size(&from)), (to.placement(), size(&to)));
        let commit = commit_of(&from, &to, share, &mixed);
        Self {
            from,
            to,
            share,
            program,
            mixed,
            commit,
        }
    }
}

/// A commit that moves whenever what the crossfade draws may have: either
/// result's commit, the share, the box, or where `to` is read.
fn commit_of(from: &EffectElement, to: &EffectElement, share: f32, mixed: &Mixed) -> CommitCounter {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for commit in [from.current_commit(), to.current_commit()] {
        commit
            .distance(Some(CommitCounter::default()))
            .hash(&mut hasher);
    }
    share.to_bits().hash(&mut hasher);
    to.placement().alpha.to_bits().hash(&mut hasher);
    (
        mixed.dst.loc.x,
        mixed.dst.loc.y,
        mixed.dst.size.w,
        mixed.dst.size.h,
    )
        .hash(&mut hasher);
    for value in mixed.to_map {
        value.to_bits().hash(&mut hasher);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a hash, of which any bits will do"
    )]
    let commit = hasher.finish() as usize;
    CommitCounter::from(commit)
}

impl Element for CrossfadeElement {
    fn id(&self) -> &Id {
        self.to.id()
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
        self.to.placement().alpha
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
        let (from, to) = (self.from.texture().tex_id(), self.to.texture().tex_id());
        // SAFETY: inside the frame's own context, both textures this
        // renderer's; smithay binds `from` on unit 0 itself, and unit 1 is
        // given back below.
        frame.with_context(|gl| unsafe { bind(gl, from, to) })?;
        let mixed = self.mixed;
        let drawn = frame.render_texture_from_to(
            self.from.texture(),
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha(),
            Some(&self.program),
            &[
                Uniform::new(CROSSFADE_TO_UNIFORM, UniformValue::_1i(1)),
                Uniform::new(CROSSFADE_SHARE_UNIFORM, self.share),
                Uniform::new(CROSSFADE_TO_MAP_UNIFORM, mixed.to_map),
                Uniform::new(CROSSFADE_FROM_BOUNDS_UNIFORM, mixed.from_bounds),
                Uniform::new(CROSSFADE_TO_BOUNDS_UNIFORM, mixed.to_bounds),
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

/// `to` on unit 1, and both read linear and clamped at their edges, which a
/// texture made by `create_buffer` is not until told (`run.rs`); unit 0
/// left active and empty, as smithay binds it itself.
///
/// # Safety
/// Inside a current context whose names `from` and `to` are.
unsafe fn bind(gl: &ffi::Gles2, from: ffi::types::GLuint, to: ffi::types::GLuint) {
    let linear = i32::try_from(ffi::LINEAR).unwrap_or_default();
    let edge = i32::try_from(ffi::CLAMP_TO_EDGE).unwrap_or_default();
    // SAFETY: the caller's contract.
    unsafe {
        gl.ActiveTexture(ffi::TEXTURE1);
        gl.BindTexture(ffi::TEXTURE_2D, to);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, linear);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, linear);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, edge);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, edge);
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, from);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, edge);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, edge);
        gl.BindTexture(ffi::TEXTURE_2D, 0);
    }
}

/// Unit 1 empty again and unit 0 active: what smithay expects to find.
///
/// # Safety
/// Inside a current context.
unsafe fn unbind(gl: &ffi::Gles2) {
    // SAFETY: the caller's contract.
    unsafe {
        gl.ActiveTexture(ffi::TEXTURE1);
        gl.BindTexture(ffi::TEXTURE_2D, 0);
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
        let got = mixed((placed, (200, 100)), (placed, (200, 100)));
        assert_eq!(got.dst, rect(40, 30, 200, 100));
        assert_eq!(
            got.src,
            Rectangle::<f64, Buffer>::new((0.0, 0.0).into(), (200.0, 100.0).into())
        );
        assert_eq!(got.to_map, [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(got.from_bounds, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(got.to_bounds, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(got.cut, [1.0, 1.0]);
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
        let got = mixed((from, (100, 50)), (to, (120, 70)));
        assert_eq!(got.dst, rect(90, 90, 120, 70));
        assert_eq!(
            got.src,
            Rectangle::<f64, Buffer>::new((-10.0, -10.0).into(), (120.0, 70.0).into())
        );
        assert!(close(
            to_at(got.to_map, (0.0, 0.0)),
            (10.0 / 120.0, 10.0 / 70.0)
        ));
        assert!(close(
            to_at(got.to_map, (1.0, 1.0)),
            (110.0 / 120.0, 60.0 / 70.0)
        ));
        assert_eq!(got.from_bounds, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(got.cut, [1.0, 0.0]);
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
        let got = mixed((from, (296, 196)), (to, (148, 98)));
        assert_eq!(got.from_bounds, [0.5, 0.0, 1.0, 0.5]);
        assert!(close(to_at(got.to_map, (0.5, 0.0)), (0.0, 0.0)));
        assert!(close(to_at(got.to_map, (1.0, 0.5)), (1.0, 1.0)));
        assert_eq!(got.cut, [0.0, 0.0]);
    }
}
