//! An effect's result in its slot: a texture over its padded box, cut by the
//! part's mask through `fragment::MASKED_TEXTURE` (\[16\] §2's last stage).
//!
//! `pass::Rounded` generalised (deleted by Phase 0 Task 23b). The program and
//! its uniforms are scoped to the one `render_texture_from_to` call, not set
//! as the frame's override, for the reason that element gave: an override
//! would reach every texture drawn after it in the frame.
//!
//! Smithay and `solium_effects` only, so `dev/wirecheck` includes this file
//! by `#[path]` and its case 12h draws this element on the GPU: the uniforms
//! `draw` hands the program, which no unit test can see drawn.

use smithay::backend::renderer::{
    Texture as _,
    element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
    gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform},
    utils::{CommitCounter, OpaqueRegions},
};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};
use solium_effects::fragment::{Corners, MASK_RADII_UNIFORM, MASK_RECT_UNIFORM, MASK_SIZE_UNIFORM};

/// Where and how a result is placed: its padded box on the output, the
/// part's mask in that box's physical pixels from its corner, the part's
/// alpha, and the part of the texture drawn (`crop`, in its pixels; `None`
/// draws it whole). `tests::the_mask_is_measured_in_the_placements_own_pixels`,
/// `tests::a_cropped_placement_draws_only_its_crop`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub(crate) dst: Rectangle<i32, Physical>,
    pub(crate) mask: Option<(Rectangle<f64, Physical>, Corners)>,
    pub(crate) alpha: f32,
    pub(crate) crop: Option<Rectangle<f64, Buffer>>,
}

impl Placement {
    /// The whole texture at `dst`, cut by `mask`, at `alpha`.
    /// `tests::a_cropped_placement_draws_only_its_crop`.
    pub(crate) fn of(
        dst: Rectangle<i32, Physical>,
        mask: Option<(Rectangle<f64, Physical>, Corners)>,
        alpha: f32,
    ) -> Self {
        Self {
            dst,
            mask,
            alpha,
            crop: None,
        }
    }

    /// The same placement drawing only `crop` of its texture: a memo,
    /// cropped to one part's padded box (`render::run_slots`).
    /// `tests::a_cropped_placement_draws_only_its_crop`.
    pub(crate) fn cropped(self, crop: Rectangle<f64, Buffer>) -> Self {
        Self {
            crop: Some(crop),
            ..self
        }
    }

    /// The same result, its crop kept, at `dst`, cut by `mask`, at `alpha`:
    /// which part of the texture is the result is the result's own.
    /// `tests::a_cropped_result_placed_again_keeps_its_crop`.
    pub(crate) fn at(
        self,
        dst: Rectangle<i32, Physical>,
        mask: Option<(Rectangle<f64, Physical>, Corners)>,
        alpha: f32,
    ) -> Self {
        Self {
            dst,
            mask,
            alpha,
            ..self
        }
    }
}

/// What of a `size` texture a placement draws: its crop, or the whole
/// texture. `tests::a_cropped_placement_draws_only_its_crop`.
pub(crate) fn src_of(placement: Placement, size: (i32, i32)) -> Rectangle<f64, Buffer> {
    placement
        .crop
        .unwrap_or_else(|| Rectangle::from_size((f64::from(size.0), f64::from(size.1)).into()))
}

/// The commit after this placement: moved when the chain re-ran or the
/// placement differs.
/// `tests::an_output_keeps_its_id_and_moves_its_commit_only_when_rerun_or_moved`.
pub(crate) fn commit_for(
    commit: &mut CommitCounter,
    was: Option<Placement>,
    now: Placement,
    rerun: bool,
) -> CommitCounter {
    if rerun || was != Some(now) {
        commit.increment();
    }
    *commit
}

/// Nothing: a result may be translucent anywhere.
/// `tests::an_output_claims_nothing_opaque`.
pub(crate) fn opaque() -> OpaqueRegions<i32, Physical> {
    OpaqueRegions::default()
}

/// `MASKED_TEXTURE`'s three uniforms, as `draw` hands them over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MaskUniforms {
    pub(crate) rect: [f32; 4],
    /// `(tl, tr, bl, br)`, `Corners`' order and the order the program picks.
    pub(crate) radii: [f32; 4],
    pub(crate) size: [f32; 2],
}

/// The masked program's uniforms for a placement of a `texture`-sized
/// result, measured in the placement's box and not in the result's own
/// pixels, so a result drawn over a box of another size is cut where the
/// placement says; `None` with no mask, drawn by smithay's own texture
/// program. The program finds a pixel at `v_coords * mask_size -
/// mask_rect.xy`, and smithay gives the box's pixel `p` the coordinate
/// `(src.loc + p * src.size / dst) / texture`: with the whole texture as
/// `src`, `mask_size` is the box's size; through a crop, the texture's size
/// and the mask's corner are scaled by `dst / crop.size` and the corner
/// moved by the crop's, so `p` still lands at `p - mask.loc`.
/// `tests::the_mask_is_measured_in_the_placements_own_pixels`,
/// `tests::through_a_crop_the_mask_stays_in_the_placements_own_pixels`,
/// `tests::with_no_mask_the_result_is_drawn_whole` and wirecheck's case 12h.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a mask in a box's pixels, as the f32 the uniforms take"
)]
pub(crate) fn mask_uniforms(placement: Placement, texture: (i32, i32)) -> Option<MaskUniforms> {
    let (rect, radii) = placement.mask?;
    let dst = (
        f64::from(placement.dst.size.w),
        f64::from(placement.dst.size.h),
    );
    let (size, shift) = match placement.crop {
        None => (dst, (0.0, 0.0)),
        Some(crop) => (
            (
                f64::from(texture.0) * dst.0 / crop.size.w,
                f64::from(texture.1) * dst.1 / crop.size.h,
            ),
            (
                crop.loc.x * dst.0 / crop.size.w,
                crop.loc.y * dst.1 / crop.size.h,
            ),
        ),
    };
    Some(MaskUniforms {
        rect: [
            (rect.loc.x + shift.0) as f32,
            (rect.loc.y + shift.1) as f32,
            rect.size.w as f32,
            rect.size.h as f32,
        ],
        radii: [
            radii.top_left as f32,
            radii.top_right as f32,
            radii.bottom_left as f32,
            radii.bottom_right as f32,
        ],
        size: [size.0 as f32, size.1 as f32],
    })
}

/// A result texture placed in its slot, drawn through the masked program.
#[derive(Clone, Debug)]
pub(crate) struct EffectElement {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    program: GlesTexProgram,
    placement: Placement,
}

impl EffectElement {
    pub(crate) fn new(
        id: Id,
        commit: CommitCounter,
        texture: GlesTexture,
        program: GlesTexProgram,
    ) -> Self {
        Self {
            id,
            commit,
            texture,
            program,
            placement: Placement::of(Rectangle::default(), None, 1.0),
        }
    }

    /// The same result at `dst`, cut by `mask` (in `dst`'s physical pixels
    /// from its corner; `None` draws the texture whole), at `alpha`, its
    /// crop kept. `tests::a_cropped_result_placed_again_keeps_its_crop` and
    /// wirecheck's case 12h.
    pub(crate) fn at(
        &self,
        dst: Rectangle<i32, Physical>,
        mask: Option<(Rectangle<f64, Physical>, Corners)>,
        alpha: f32,
    ) -> Self {
        Self {
            placement: self.placement.at(dst, mask, alpha),
            ..self.clone()
        }
    }

    /// The same result drawing only `crop` of its texture: a memo, cropped
    /// to one part's padded box (`render::run_slots`), the crop kept by
    /// every later [`Self::at`]. Wirecheck's case 12h.
    pub(crate) fn cropped(&self, crop: Rectangle<f64, Buffer>) -> Self {
        Self {
            placement: self.placement.cropped(crop),
            ..self.clone()
        }
    }

    /// Its texture, which a crossfade reads it from (`effect::crossfade`).
    pub(crate) fn texture(&self) -> &GlesTexture {
        &self.texture
    }

    /// Where and how it is placed, which a crossfade places it by.
    pub(crate) fn placement(&self) -> Placement {
        self.placement
    }
}

impl Element for EffectElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        let size = self.texture.size();
        src_of(self.placement, (size.w, size.h))
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.placement.dst
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        opaque()
    }

    fn alpha(&self) -> f32 {
        self.placement.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for EffectElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let alpha = self.placement.alpha;
        let size = self.texture.size();
        let Some(mask) = mask_uniforms(self.placement, (size.w, size.h)) else {
            return frame.render_texture_from_to(
                &self.texture,
                src,
                dst,
                damage,
                opaque_regions,
                Transform::Normal,
                alpha,
                None,
                &[],
            );
        };
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            alpha,
            Some(&self.program),
            &[
                Uniform::new(MASK_RECT_UNIFORM, mask.rect),
                Uniform::new(MASK_RADII_UNIFORM, mask.radii),
                Uniform::new(MASK_SIZE_UNIFORM, mask.size),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::{Physical, Rectangle};
    use solium_effects::fragment::Corners;

    use super::{MaskUniforms, Placement};

    /// **An output keeps its id, and moves its commit only when re-run or
    /// moved**: placing the same result at the same rect twice is no damage.
    #[test]
    fn an_output_keeps_its_id_and_moves_its_commit_only_when_rerun_or_moved() {
        let mut commit = CommitCounter::default();
        let rect: Rectangle<i32, Physical> = Rectangle::new((10, 10).into(), (100, 80).into());
        let a = Placement::of(rect, None, 1.0);
        let b = Placement::of(rect, None, 1.0);
        assert_eq!(
            super::commit_for(&mut commit, None, a, false),
            super::commit_for(&mut commit, Some(a), b, false),
            "nothing changed"
        );
        let moved = Placement::of(Rectangle::new((11, 10).into(), (100, 80).into()), None, 1.0);
        let before = commit;
        assert_ne!(
            super::commit_for(&mut commit, Some(b), moved, false),
            before,
            "moved"
        );
        let before = commit;
        assert_ne!(
            super::commit_for(&mut commit, Some(moved), moved, true),
            before,
            "re-run"
        );
    }

    /// **An output claims nothing opaque**: what is under a translucent
    /// result must still be drawn.
    #[test]
    fn an_output_claims_nothing_opaque() {
        assert!(super::opaque().is_empty());
    }

    /// **The mask is measured in the placement's own pixels**, not the
    /// result's: a result drawn over a box of another size (a second monitor
    /// at another scale) is cut where the placement put the mask. The four
    /// radii keep `Corners`' order, the order `MASKED_TEXTURE` picks them in.
    #[test]
    fn the_mask_is_measured_in_the_placements_own_pixels() {
        let placement = Placement::of(
            Rectangle::new((10, 20).into(), (200, 100).into()),
            Some((
                Rectangle::new((8.0, 8.0).into(), (184.0, 84.0).into()),
                Corners {
                    top_left: 1.0,
                    top_right: 2.0,
                    bottom_left: 3.0,
                    bottom_right: 4.0,
                },
            )),
            1.0,
        );
        assert_eq!(
            super::mask_uniforms(placement, (100, 50)),
            Some(MaskUniforms {
                rect: [8.0, 8.0, 184.0, 84.0],
                radii: [1.0, 2.0, 3.0, 4.0],
                size: [200.0, 100.0],
            })
        );
    }

    /// **Through a crop, the mask stays in the placement's own pixels**:
    /// `MASKED_TEXTURE` finds a pixel at `v_coords * mask_size -
    /// mask_rect.xy`, and smithay's texture matrix gives the box's pixel `p`
    /// the coordinate `(crop.loc + p * crop.size / dst) / texture`, so the
    /// uniforms map the crop back out. For an off-centre mask, with a crop
    /// drawn 1:1 and one drawn at twice its size, `p` lands at `p - mask.loc`.
    #[test]
    fn through_a_crop_the_mask_stays_in_the_placements_own_pixels() {
        let mask = (
            Rectangle::new((10.0, 40.0).into(), (90.0, 50.0).into()),
            Corners::all(10.0),
        );
        for (texture, crop) in [
            (
                (296, 196),
                Rectangle::new((148.0, 0.0).into(), (148.0, 98.0).into()),
            ),
            (
                (148, 98),
                Rectangle::new((74.0, 0.0).into(), (74.0, 49.0).into()),
            ),
        ] {
            let placement = Placement::of(
                Rectangle::new((7, 9).into(), (148, 98).into()),
                Some(mask),
                1.0,
            )
            .cropped(crop);
            let uniforms = super::mask_uniforms(placement, texture).expect("a mask");
            assert_eq!(
                uniforms,
                MaskUniforms {
                    rect: [158.0, 40.0, 90.0, 50.0],
                    radii: [10.0; 4],
                    size: [296.0, 196.0],
                }
            );
            for (x, y) in [(0.5, 0.5), (75.5, 60.5), (5.5, 94.5), (147.5, 97.5)] {
                let v = (
                    (crop.loc.x + x * crop.size.w / 148.0) / f64::from(texture.0),
                    (crop.loc.y + y * crop.size.h / 98.0) / f64::from(texture.1),
                );
                let at = (
                    v.0 * f64::from(uniforms.size[0]) - f64::from(uniforms.rect[0]),
                    v.1 * f64::from(uniforms.size[1]) - f64::from(uniforms.rect[1]),
                );
                assert!(
                    (at.0 - (x - 10.0)).abs() < 1e-3 && (at.1 - (y - 40.0)).abs() < 1e-3,
                    "{texture:?} {crop:?}: ({x}, {y}) lands at {at:?}"
                );
            }
        }
    }

    /// **With no mask the result is drawn whole**, by smithay's own texture
    /// program: an effect that reads `shape` owns its edges, and nothing
    /// stands in for "no mask" with a rectangle too big for `mediump`.
    #[test]
    fn with_no_mask_the_result_is_drawn_whole() {
        let placement = Placement::of(Rectangle::from_size((64, 32).into()), None, 0.5);
        assert_eq!(super::mask_uniforms(placement, (64, 32)), None);
    }

    /// **A cropped result placed again keeps its crop**: the walk places a
    /// slot's result at its part on every pass (`EffectElement::at`), and
    /// which part of the texture is the result is the result's own, not
    /// the place's.
    #[test]
    fn a_cropped_result_placed_again_keeps_its_crop() {
        let crop = Rectangle::new((200.0, 120.0).into(), (100.0, 40.0).into());
        let dst = Rectangle::new((5, 6).into(), (100, 40).into());
        let placed = Placement::of(Rectangle::from_size((100, 40).into()), None, 1.0)
            .cropped(crop)
            .at(dst, None, 0.5);
        assert_eq!(
            placed,
            Placement {
                dst,
                mask: None,
                alpha: 0.5,
                crop: Some(crop),
            }
        );
    }

    /// **A cropped placement draws only its crop**: `src` is the crop, and
    /// with none it is the whole texture, as before.
    #[test]
    fn a_cropped_placement_draws_only_its_crop() {
        let dst = Rectangle::new((0, 0).into(), (100, 40).into());
        let plain = Placement::of(dst, None, 1.0);
        assert_eq!(plain.crop, None);
        let crop = Rectangle::new((200.0, 120.0).into(), (100.0, 40.0).into());
        let cropped = plain.cropped(crop);
        assert_eq!(super::src_of(cropped, (2560, 1440)), crop);
        assert_eq!(
            super::src_of(plain, (2560, 1440)),
            Rectangle::from_size((2560.0, 1440.0).into())
        );
    }

    /// **A moved crop moves the commit**, so the damage under a window that
    /// moves over a memo is redrawn; an unmoved one keeps it.
    #[test]
    fn a_moved_crop_moves_the_commit() {
        let dst = Rectangle::new((0, 0).into(), (100, 40).into());
        let a = Placement::of(dst, None, 1.0)
            .cropped(Rectangle::new((0.0, 0.0).into(), (100.0, 40.0).into()));
        let b = a.cropped(Rectangle::new((4.0, 0.0).into(), (100.0, 40.0).into()));
        let mut commit = CommitCounter::default();
        let first = super::commit_for(&mut commit, None, a, false);
        assert_eq!(super::commit_for(&mut commit, Some(a), a, false), first);
        assert_ne!(super::commit_for(&mut commit, Some(a), b, false), first);
    }
}
