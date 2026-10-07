//! The programs a frame draws through, and which effects this renderer runs.
//!
//! An effect that declares `Inputs::Nothing` is an ordinary element. Rounding,
//! the one effect this build makes, declares `Inputs::Inline` and is drawn by
//! `crate::clip`, each client surface through the clipped programs here, with
//! no capture (`render::tests::a_style_with_a_radius_is_drawn_inline`). An
//! effect that reads what is beneath a node is refused out loud
//! (`tests::an_effect_that_cannot_be_run_is_named_once_and_not_every_frame`).
//!
//! **Programs are compiled in `render::prepare` and never in
//! `render::elements`, and that is not a preference.** `elements` runs with
//! the output's buffer already bound on the nested backend (`winit.rs`) and
//! inside `offscreen::Screens::draw` on the multi-monitor one, and a compile's
//! `make_current` there is the frozen-compositor failure `render::Prepared`
//! describes. So `prepare` compiles, and `elements` only reads what it
//! compiled (`Programs::clip_compiled`).

use smithay::backend::renderer::gles::{GlesPixelProgram, GlesRenderer, GlesTexProgram};
use solium_effects::fragment::{
    CLIPPED_SOLID, CLIPPED_SURFACE, Corners, Effect, Inputs, MASKED_TEXTURE,
};

mod uniforms;
pub(crate) use uniforms::registration;

/// Whether this renderer can run an effect that reads `inputs` at all.
///
/// An exhaustive match, so a new `Inputs` variant stops the build here rather
/// than being run, or skipped, by accident. [`Inputs::Nothing`] is runnable: it
/// is an ordinary element drawn over what is there
/// (`tests::an_effect_reading_a_backdrop_cannot_be_run`).
const fn runnable(inputs: Inputs) -> bool {
    match inputs {
        Inputs::Nothing | Inputs::Inline | Inputs::SelfTexture => true,
        // Nothing composites what is beneath a node into anything this file
        // could sample. Answering `true` here would be the exact failure
        // `fragment::Inputs::Backdrop` names: a blur that renders as no blur,
        // which looks like a style that failed to load and is never reported
        // as a compositor bug.
        Inputs::Backdrop => false,
    }
}

/// The first effect here that this renderer cannot run.
///
/// Asked by `render::prepare` of every pane it does not capture, which says so
/// once ([`Programs::refuse`]): not drawing [`Inputs::Backdrop`] is right --
/// there is no backdrop to read -- but not drawing it *silently* is what
/// `fragment::Inputs::Backdrop` forbids.
///
/// Only reachable with a non-empty effect list. That is not the same as "never
/// on an unstyled machine" any more, and the weaker claim is the true one:
/// `style::load` pushes nothing for an absent or zero `client.radius`, which is
/// nine of the eleven shipped bundles -- but `panes/rounded/` and
/// `panes/flush/` each declare `client.radius: 12`, so a session using either
/// walks a one-element list here.
///
/// **Nothing can construct an effect this returns today**, because `Effect` has
/// one variant and it reads `Inline`. That is why the guarantee is
/// [`runnable`]'s exhaustive match and not this walk: a fourth `Inputs` variant
/// is `error[E0004]` there, whether or not anybody remembers this function.
pub(crate) fn refused(effects: &[Effect]) -> Option<Effect> {
    effects
        .iter()
        .copied()
        .find(|effect| !effect.is_none_effect() && !runnable(effect.inputs()))
}

/// The declared radii, in the **physical** pixels the clipped programs
/// measure in.
///
/// **This is the seam `fragment::RADIUS_UNIFORM` names, and it is the whole of
/// it.** [`Effect::radii`] is logical, because a style writes `radius: 12`
/// into a `Pane.qml` and cannot know which monitor the window will land on;
/// the clipped programs measure in the client's own physical pixels
/// (`clip::input_to_geo`), at the scale of the monitor it is drawn on.
///
/// The two numbers are equal at scale 1, which is why getting this wrong is
/// perfect on the machine it was written on and wrong on every HiDPI one --
/// and wrong *differently* on each screen of a desk with two scales, since
/// `scale` here is the drawing monitor's and not a constant.
///
/// **Four multiplies and not one.** Scaling a single number -- the largest,
/// say -- and copying it to the other three is the same defect one step in:
/// exactly right whenever the corners agree, and wrong by the ratio between
/// them the moment they do not, which is the case this whole change exists
/// for. `Corners` keeps its own field order here and everywhere, because that
/// order is what the shader indexes `corner_radius` by.
pub(crate) fn physical_radii(effect: Effect, scale: f64) -> Corners {
    let radii = effect.radii();
    Corners {
        top_left: radii.top_left * scale,
        top_right: radii.top_right * scale,
        bottom_left: radii.bottom_left * scale,
        bottom_right: radii.bottom_right * scale,
    }
}

/// Compile once and keep it, or latch the failure and never try again.
/// `tests::a_program_that_will_not_compile_is_tried_once`.
fn once<'a, T>(
    slot: &'a mut Option<T>,
    failed: &mut bool,
    compile: impl FnOnce() -> Option<T>,
) -> Option<&'a T> {
    if slot.is_none() && !*failed {
        match compile() {
            Some(made) => *slot = Some(made),
            None => *failed = true,
        }
    }
    slot.as_ref()
}

/// The two programs a clipped client surface is drawn through: one for
/// textures and one for single-pixel buffers. A pair rather than a rounding
/// type: a user's inline effect (a tint, a dim) is the same element with
/// another pair (X1.4). Wirecheck's case 11h draws through both.
#[derive(Clone, Debug)]
pub(crate) struct ClipPrograms {
    pub(crate) texture: GlesTexProgram,
    pub(crate) solid: GlesPixelProgram,
}

/// The compiled fragment programs, one of each, for the life of the renderer.
///
/// Compiling a shader is not a per-frame cost anybody should pay, and
/// `compile_custom_texture_shader` calls `make_current` -- which is not free
/// and, worse, is exactly the kind of thing that has broken Qt's stale
/// thread-local `currentContext` five separate times in this codebase.
///
/// **That hazard does not reach this call, and the reason is worth writing
/// down because it is not obvious from the call site.** Two things hold it:
///
/// * It cannot run inside a live `GlesFrame`. `GlesFrame` holds
///   `&'frame mut GlesRenderer` (smithay `gles/mod.rs:329`), so `&mut
///   GlesRenderer` in this signature is a borrow the frame already has. The
///   invariant `qml::no_frame_in_flight` states by convention is, for this one
///   call, a compile error -- which is why there is no runtime assertion here.
///   And it cannot be shortened away: `GlesFrame` has a `Drop` impl (`gles/
///   mod.rs:2966`), so the borrow lives to the end of the frame's scope rather
///   than to its last use. Checked by construction, not by reading -- a probe
///   call in `offscreen.rs`'s live-frame arm gives `error[E0499]: cannot
///   borrow *renderer as mutable more than once`, and the compiler names the
///   destructor as the reason.
/// * Between frames it is one more ordinary `GlesRenderer` entry point.
///   `import_dmabuf`, `bind`, `render` and `wait` all `make_current` the same
///   way, on every frame, and Qt's belief is corrected on the way *in* to Qt
///   rather than on the way out: `solium_qml_scene_render_gpu` and
///   `solium_qml_scene_free` open with `clear_stale_current_context`, and
///   `solium_qml_scene_rebind` with `take_the_thread`. Compiling on first use
///   adds no new kind of interleaving, only one more instance of one that is
///   already handled.
#[derive(Debug, Default)]
pub(crate) struct Programs {
    /// The warp's program (`warp/gl.rs`).
    warp: Option<crate::warp::Program>,
    /// Set once a compile has been tried and failed, so the warning is logged
    /// once rather than at sixty or two hundred and sixty hertz.
    warp_failed: bool,
    /// The clipped-surface programs, on the same terms as `warp`.
    clip: Option<ClipPrograms>,
    /// As `warp_failed`, for the clipped-surface programs.
    clip_failed: bool,
    /// The program an effect's result is drawn through
    /// (`effect::element::EffectElement`), on the same terms as `warp`.
    masked: Option<GlesTexProgram>,
    /// As `warp_failed`, for the masked program.
    masked_failed: bool,
    /// Set once an effect this renderer cannot run has been named, for the
    /// same reason and on the same terms. See [`Programs::refuse`].
    refused: bool,
}

impl Programs {
    /// The warp program, compiled on first use and between frames, for the
    /// reason this type's own doc gives. `None` means it did not compile and a
    /// deformed window is drawn flat rather than not at all (#140, §6.5 C3).
    #[expect(unsafe_code, reason = "compiling the warp's GL program")]
    pub(crate) fn warp(&mut self, renderer: &mut GlesRenderer) -> Option<crate::warp::Program> {
        once(&mut self.warp, &mut self.warp_failed, || {
            // SAFETY: `with_context` makes the renderer's context current, and
            // this runs between frames (`render::prepare`).
            match renderer.with_context(|gl| unsafe { crate::warp::Program::compile(gl) }) {
                Ok(Ok(program)) => Some(program),
                Ok(Err(why)) => {
                    tracing::warn!(
                        why,
                        "the warp shader did not compile; deformed windows will be drawn flat"
                    );
                    None
                }
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        "no context to compile the warp shader in; deformed windows will be drawn flat"
                    );
                    None
                }
            }
        })
        .copied()
    }

    /// The clipped-surface programs, compiled on first use between frames and
    /// latched (`tests::a_program_that_will_not_compile_is_tried_once`).
    /// `None` leaves a rounded window square rather than undrawn. Each is
    /// registered with the uniforms its source declares
    /// (`tests::the_registered_uniforms_are_the_declared_ones_but_smithays`);
    /// wirecheck's case 11h compiles the same two through the same
    /// [`registration`].
    pub(crate) fn clip(&mut self, renderer: &mut GlesRenderer) -> Option<&ClipPrograms> {
        once(&mut self.clip, &mut self.clip_failed, || {
            let texture = renderer
                .compile_custom_texture_shader(CLIPPED_SURFACE, &registration(CLIPPED_SURFACE));
            let solid =
                renderer.compile_custom_pixel_shader(CLIPPED_SOLID, &registration(CLIPPED_SOLID));
            match (texture, solid) {
                (Ok(texture), Ok(solid)) => Some(ClipPrograms { texture, solid }),
                (texture, solid) => {
                    tracing::warn!(
                        texture = ?texture.err(),
                        solid = ?solid.err(),
                        "the clipped-surface shaders did not compile; rounded windows will be drawn square"
                    );
                    None
                }
            }
        })
    }

    /// The clipped-surface programs if [`Programs::clip`] has compiled them,
    /// for `render::elements`, which may run with an output bound and so must
    /// never compile: `render::prepare` does, between frames, for every pane
    /// whose style declares a rounding.
    pub(crate) fn clip_compiled(&self) -> Option<&ClipPrograms> {
        self.clip.as_ref()
    }

    /// The program an effect's result is drawn through, cut by its part's
    /// mask (`effect::element`), compiled on first use between frames and
    /// latched (`tests::a_program_that_will_not_compile_is_tried_once`), with
    /// the uniforms its source declares
    /// (`tests::the_masked_programs_registration_is_its_declared_uniforms`);
    /// wirecheck's case 12h compiles the same through the same
    /// [`registration`].
    pub(crate) fn masked(&mut self, renderer: &mut GlesRenderer) -> Option<&GlesTexProgram> {
        once(&mut self.masked, &mut self.masked_failed, || {
            renderer
                .compile_custom_texture_shader(MASKED_TEXTURE, &registration(MASKED_TEXTURE))
                .map_err(|err| {
                    tracing::warn!(?err, "the masked-texture shader did not compile");
                })
                .ok()
        })
    }

    /// The masked program if [`Programs::masked`] has compiled it, for
    /// `render::elements`, which must never compile (this module's doc).
    pub(crate) fn masked_compiled(&self) -> Option<&GlesTexProgram> {
        self.masked.as_ref()
    }

    /// Say, once, that a style declares an effect this renderer cannot run.
    ///
    /// The out-loud refusal `fragment::Inputs::Backdrop` asks for. A style
    /// declaring one declares it on every frame of every window it is applied
    /// to, so this latches exactly as `clip_failed` does and for exactly
    /// the same reason: a warning at the refresh rate is how a log stops being
    /// readable at the moment somebody needs it.
    ///
    /// One latch for the whole renderer and not one per effect, which is
    /// coarse on purpose -- the cost of being coarse is a second unrunnable
    /// effect going unnamed in a session where one was already named, and the
    /// cost of being fine-grained is a table to sweep. Returns whether it said
    /// anything, because the latch is the part a test without a log subscriber
    /// can see.
    pub(crate) fn refuse(&mut self, effect: Effect) -> bool {
        if self.refused {
            return false;
        }
        self.refused = true;
        tracing::warn!(
            ?effect,
            inputs = ?effect.inputs(),
            "this style declares an effect that reads what is composited beneath \
             the window; nothing composites a backdrop, so the effect is not drawn \
             -- the window is drawn without it rather than not at all"
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The logical-to-physical seam, which is the one defect in this file a
    /// 1x machine cannot feel.**
    ///
    /// Scale 1 is deliberately not the first case and is not alone: every
    /// wrong implementation of this -- forgetting the multiply, dividing by
    /// the scale, adding it -- agrees with the right one at 1.0, which is the
    /// scale of the monitor this was written on and of the container the gate
    /// runs in. 1.5 is there as well as 2.0 because a fractional scale is the
    /// ordinary HiDPI case on this desktop, and an integer one hides a
    /// `scale as i32` that the doubling would not.
    #[test]
    fn a_declared_radius_becomes_physical_pixels_at_the_monitors_scale() {
        let rounded = Effect::rounded(Corners::all(12.0));
        assert_eq!(physical_radii(rounded, 2.0), Corners::all(24.0));
        assert_eq!(physical_radii(rounded, 1.5), Corners::all(18.0));
        assert_eq!(physical_radii(rounded, 1.0), Corners::all(12.0));
    }

    /// **All four are multiplied, and each stays its own corner.**
    ///
    /// The test above can see neither half of that: every corner of
    /// `Corners::all(12.0)` is 12, so scaling one and copying it to the other
    /// three passes it, and so does any permutation of the four. Four distinct
    /// values at a scale that is not 1 is the only shape that fails both --
    /// and 1.5 rather than 2.0, so that a doubling cannot pass for a multiply
    /// either.
    #[test]
    fn each_corner_is_scaled_and_stays_its_own_corner() {
        let declared = Corners {
            top_left: 2.0,
            top_right: 4.0,
            bottom_left: 8.0,
            bottom_right: 16.0,
        };
        assert_eq!(
            physical_radii(Effect::rounded(declared), 1.5),
            Corners {
                top_left: 3.0,
                top_right: 6.0,
                bottom_left: 12.0,
                bottom_right: 24.0,
            },
            "a corner that swaps places is a window rounded at the wrong end, \
             and at 1x every wrong scaling agrees with the right one"
        );
    }

    /// What this renderer can run at all. [`Inputs::Nothing`] answers yes, and
    /// an implementation that refused what reads nothing would refuse every
    /// ordinary element.
    #[test]
    fn an_effect_reading_a_backdrop_cannot_be_run() {
        assert!(!runnable(Inputs::Backdrop));
        assert!(runnable(Inputs::SelfTexture));
        assert!(
            runnable(Inputs::Nothing),
            "an effect that reads nothing is an ordinary element, not a refusal"
        );
    }

    /// An effect drawn inline, surface by surface through a program of its
    /// own (`fragment::CLIPPED_SURFACE`), is one this renderer runs: refused,
    /// it would be named as unrunnable and the window drawn without it.
    #[test]
    fn an_effect_drawn_inline_can_be_run() {
        assert!(runnable(Inputs::Inline));
    }

    /// Nothing in this build can construct an effect that reads a backdrop --
    /// `Effect` has one variant and it reads `Inline` -- so this is the
    /// only half of [`refused`] a test can reach, and it is asserted because
    /// the expensive way to be wrong is the other direction: a `refused` that
    /// answered `Some` for the rounded corners a styled window declares would
    /// refuse the feature this plan exists to add.
    #[test]
    fn the_effects_this_build_can_make_are_not_refused() {
        assert_eq!(refused(&[]), None);
        assert_eq!(refused(&[Effect::rounded(Corners::all(12.0))]), None);
        assert_eq!(refused(&[Effect::rounded(Corners::all(0.0))]), None);
    }

    /// Said once, not at the refresh rate.
    ///
    /// A style declaring an unrunnable effect declares it on every frame of
    /// every window it is applied to. Without the latch this is a warning per
    /// window per frame, which is how a log stops being readable at the moment
    /// somebody needs it -- the argument `clip_failed` already makes, and
    /// the reason the return value exists at all: there is no log subscriber
    /// in a unit test, so the latch is the only observable part.
    #[test]
    fn an_effect_that_cannot_be_run_is_named_once_and_not_every_frame() {
        let mut programs = Programs::default();
        let effect = Effect::rounded(Corners::all(12.0));
        assert!(programs.refuse(effect), "the first refusal says so");
        assert!(!programs.refuse(effect), "and the second says nothing");
        assert!(
            !programs.refuse(Effect::rounded(Corners::all(4.0))),
            "nor a different one"
        );
    }

    /// **A program that will not compile is tried once**, not on every frame:
    /// the thing that failed is a string against a driver, and it will fail the
    /// same way next frame. The warp's and the clipped programs' latch.
    #[test]
    fn a_program_that_will_not_compile_is_tried_once() {
        let (mut slot, mut failed, mut tries) = (None::<u32>, false, 0);
        for _ in 0..3 {
            assert!(
                super::once(&mut slot, &mut failed, || {
                    tries += 1;
                    None
                })
                .is_none()
            );
        }
        assert_eq!(tries, 1, "a failed compile was tried again");
        let (mut slot, mut failed) = (None::<u32>, false);
        assert_eq!(
            super::once(&mut slot, &mut failed, || Some(7)).copied(),
            Some(7)
        );
        assert_eq!(
            super::once(&mut slot, &mut failed, || Some(8)).copied(),
            Some(7),
            "compiled once and kept"
        );
    }

    /// **The masked program's registration is its declared uniforms**: what
    /// `Programs::masked` compiles `MASKED_TEXTURE` with, and wirecheck's
    /// case 12h with it, names the mask's rectangle, radii and size with the
    /// types the source declares (a `vec2` registered as `_1f` would leave
    /// every fragment's mask unset, #94).
    #[test]
    fn the_masked_programs_registration_is_its_declared_uniforms() {
        use smithay::backend::renderer::gles::UniformType;
        use solium_effects::fragment::{
            MASK_RADII_UNIFORM, MASK_RECT_UNIFORM, MASK_SIZE_UNIFORM, MASKED_TEXTURE,
        };
        let registered: Vec<_> = super::registration(MASKED_TEXTURE)
            .into_iter()
            .map(|uniform| (uniform.name.into_owned(), uniform.type_))
            .collect();
        assert_eq!(
            registered,
            vec![
                (MASK_RECT_UNIFORM.to_owned(), UniformType::_4f),
                (MASK_RADII_UNIFORM.to_owned(), UniformType::_4f),
                (MASK_SIZE_UNIFORM.to_owned(), UniformType::_2f),
            ]
        );
    }

    /// **The registered uniforms are the declared ones but smithay's**, with
    /// the declared types: #94's defect (a `vec4` registered as `_1f`) cannot
    /// be written again, because nothing is written by hand. Every source
    /// registered through [`registration`]: the clipped pair, and wirecheck's
    /// case 11's `ROUNDED_CORNERS`.
    #[test]
    fn the_registered_uniforms_are_the_declared_ones_but_smithays() {
        use smithay::backend::renderer::gles::UniformType;
        for source in [
            solium_effects::fragment::CLIPPED_SURFACE,
            solium_effects::fragment::CLIPPED_SOLID,
            solium_effects::fragment::ROUNDED_CORNERS,
        ] {
            let registered = super::registration(source);
            let declared: Vec<_> = solium_effects::glsl::uniforms(source)
                .into_iter()
                .filter(|each| !super::uniforms::SMITHAYS.contains(&each.name.as_str()))
                .collect();
            assert_eq!(registered.len(), declared.len(), "{source}");
            for (name, uniform) in registered.iter().zip(&declared) {
                assert_eq!(name.name, uniform.name);
                let expected = match uniform.ty {
                    solium_effects::glsl::Glsl::Float => UniformType::_1f,
                    solium_effects::glsl::Glsl::Vec2 => UniformType::_2f,
                    solium_effects::glsl::Glsl::Vec3 => UniformType::_3f,
                    solium_effects::glsl::Glsl::Vec4 => UniformType::_4f,
                    solium_effects::glsl::Glsl::Mat3 => UniformType::Matrix3x3,
                    other => panic!("{other:?} is not registered"),
                };
                assert_eq!(name.type_, expected, "{}", uniform.name);
            }
        }
    }
}
