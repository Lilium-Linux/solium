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
/// part's mask in that box's physical pixels from its corner, and the part's
/// alpha. `tests::the_mask_is_measured_in_the_placements_own_pixels`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub(crate) dst: Rectangle<i32, Physical>,
    pub(crate) mask: Option<(Rectangle<f64, Physical>, Corners)>,
    pub(crate) alpha: f32,
}

impl Placement {
    pub(crate) fn of(
        dst: Rectangle<i32, Physical>,
        mask: Option<(Rectangle<f64, Physical>, Corners)>,
        alpha: f32,
    ) -> Self {
        Self { dst, mask, alpha }
    }
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

/// The masked program's uniforms for a placement, measured in the
/// placement's box and not in the result's own pixels, so a result drawn
/// over a box of another size is cut where the placement says; `None` with
/// no mask, drawn by smithay's own texture program.
/// `tests::the_mask_is_measured_in_the_placements_own_pixels`,
/// `tests::with_no_mask_the_result_is_drawn_whole` and wirecheck's case 12h.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a mask in a box's pixels, as the f32 the uniforms take"
)]
pub(crate) fn mask_uniforms(placement: Placement) -> Option<MaskUniforms> {
    let (rect, radii) = placement.mask?;
    Some(MaskUniforms {
        rect: [
            rect.loc.x as f32,
            rect.loc.y as f32,
            rect.size.w as f32,
            rect.size.h as f32,
        ],
        radii: [
            radii.top_left as f32,
            radii.top_right as f32,
            radii.bottom_left as f32,
            radii.bottom_right as f32,
        ],
        size: [placement.dst.size.w as f32, placement.dst.size.h as f32],
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
    /// from its corner; `None` draws the texture whole), at `alpha`.
    /// Wirecheck's case 12h.
    pub(crate) fn at(
        &self,
        dst: Rectangle<i32, Physical>,
        mask: Option<(Rectangle<f64, Physical>, Corners)>,
        alpha: f32,
    ) -> Self {
        Self {
            placement: Placement::of(dst, mask, alpha),
            ..self.clone()
        }
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
        Rectangle::from_size((f64::from(size.w), f64::from(size.h)).into())
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
        let Some(mask) = mask_uniforms(self.placement) else {
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
            super::mask_uniforms(placement),
            Some(MaskUniforms {
                rect: [8.0, 8.0, 184.0, 84.0],
                radii: [1.0, 2.0, 3.0, 4.0],
                size: [200.0, 100.0],
            })
        );
    }

    /// **With no mask the result is drawn whole**, by smithay's own texture
    /// program: an effect that reads `shape` owns its edges, and nothing
    /// stands in for "no mask" with a rectangle too big for `mediump`.
    #[test]
    fn with_no_mask_the_result_is_drawn_whole() {
        let placement = Placement::of(Rectangle::from_size((64, 32).into()), None, 0.5);
        assert_eq!(super::mask_uniforms(placement), None);
    }
}
