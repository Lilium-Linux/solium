//! Passes: what an effect reading its own node costs, and where it is paid.
//!
//! An effect that declares `Inputs::Nothing` never reaches this file. One that
//! declares `Inputs::SelfTexture` cannot be a single element in a flat list,
//! because it needs the node's pixels before it can draw: the client's
//! surfaces are rendered into a texture of their own, and *that* is drawn,
//! through a fragment program, in the client's place.
//!
//! The capture is [`crate::offscreen::capture_client`], a sibling of the one
//! the genie already uses, and it keeps its texture on the pane rather than
//! allocating one a frame. That was made a prerequisite of this work rather
//! than a follow-up for exactly this reason: an effect system multiplies a
//! per-frame allocation by the number of animating windows.
//!
//! **The capture happens in `render::prepare` and never in `render::elements`,
//! and that is not a preference.** A capture binds a framebuffer of its own,
//! and `elements` runs with the output's buffer already bound on the nested
//! backend (`winit.rs`) and inside `offscreen::Screens::draw` on the
//! multi-monitor one -- a bind underneath a bind redirects the whole frame
//! into a texture nobody shows, which reads as a frozen compositor happily
//! reporting successful frames. So `prepare` captures, [`Pass`] carries the
//! result to every output that draws the window, and `elements` only places
//! it.

use smithay::{
    backend::renderer::{
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::{
            GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
            UniformType,
        },
        utils::{CommitCounter, OpaqueRegions},
    },
    utils::{Buffer as BufferCoords, Physical, Rectangle, Scale, Size, Transform},
};
use solium_effects::fragment::{Effect, Inputs, RADIUS_UNIFORM, ROUNDED_CORNERS, SIZE_UNIFORM};

/// Whether this node's effects need the node rendered to a texture first, and
/// which effect wants it.
///
/// `None` is the answer for every window on a machine nobody has styled, and
/// it is asked once per pane per frame, so it is a slice walk and nothing
/// more. The first effect wins: one pass, one program, and a node wanting two
/// fragment effects at once is a thing to design when something wants it.
///
/// [`Inputs::Backdrop`] answers `None` here, which is correct and is *not* the
/// whole answer: nothing constructs one yet, and the day something does, an
/// effect that reads what is beneath the node must be refused out loud rather
/// than quietly skipped. That refusal is [`refused`], one function down, and
/// `render::prepare` asks it on the branch this one answers `None` on.
pub(crate) fn needs_pass(effects: &[Effect]) -> Option<Effect> {
    effects.iter().copied().find(|effect| {
        !effect.is_none_effect()
                // Spelled out rather than `== Inputs::SelfTexture`, and the
                // difference is the whole point: `Inputs` is not
                // `#[non_exhaustive]`, so an equality test lets a future effect
                // declaring `Backdrop` compile here and be silently skipped --
                // which is precisely the "blur renders as no blur and nobody
                // reports it" failure `fragment::Inputs::Backdrop` warns about.
                // A match makes that a compile error on this line instead, for
                // the same discriminant compare and no extra branch: put the
                // tripwire where the change happens. [`runnable`] is the same
                // mechanism aimed the other way -- this one asks whether a
                // pass is needed, that one whether the effect can be run at
                // all, and a new `Inputs` variant stops the build at both.
                && match effect.inputs() {
                    Inputs::SelfTexture => true,
                    Inputs::Nothing | Inputs::Backdrop => false,
                }
    })
}

/// Whether this renderer can run an effect that reads `inputs` at all.
///
/// Separate from [`needs_pass`] because they are separate questions, and
/// answering both with one match is how a third `Inputs` variant would come to
/// mean "no pass" and "fine as it is" at once. [`Inputs::Nothing`] is runnable
/// *and* wants no pass: it is an ordinary element drawn over what is there.
const fn runnable(inputs: Inputs) -> bool {
    match inputs {
        Inputs::Nothing | Inputs::SelfTexture => true,
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
/// The out-loud half of the answer [`needs_pass`] gives quietly. `needs_pass`
/// skipping [`Inputs::Backdrop`] is right -- there is no backdrop to read --
/// but skipping it *silently* is what `fragment::Inputs::Backdrop` forbids, so
/// the branch where `needs_pass` says `None` asks this and says so once.
///
/// Only reachable with a non-empty effect list, which on a machine nobody has
/// styled never happens: `style::load` pushes nothing at all for the absent or
/// zero `client.radius` every shipped bundle declares.
///
/// **Nothing can construct an effect this returns today**, because `Effect` has
/// one variant and it reads `SelfTexture`. That is why the guarantee is
/// [`runnable`]'s exhaustive match and not this walk: a fourth `Inputs` variant
/// is `error[E0004]` there, whether or not anybody remembers this function.
pub(crate) fn refused(effects: &[Effect]) -> Option<Effect> {
    effects
        .iter()
        .copied()
        .find(|effect| !effect.is_none_effect() && !runnable(effect.inputs()))
}

/// A declared radius, in the **physical** pixels the shader measures in.
///
/// **This is the seam `fragment::RADIUS_UNIFORM` names, and it is the whole of
/// it.** [`Effect::radius`] is logical, because a style writes `radius: 12`
/// into a `Pane.qml` and cannot know which monitor the window will land on;
/// the shader multiplies a normalised coordinate by `tex_size` and measures in
/// the texture's own pixels throughout, and the texture was captured at the
/// monitor's scale.
///
/// The two numbers are equal at scale 1, which is why getting this wrong is
/// perfect on the machine it was written on and wrong on every HiDPI one --
/// and wrong *differently* on each screen of a desk with two scales, since
/// `scale` here is the capturing monitor's and not a constant.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a corner radius is tens of pixels; f32 is what the uniform takes"
)]
pub(crate) fn physical_radius(effect: Effect, scale: f64) -> f32 {
    (effect.radius() * scale) as f32
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
    rounded: Option<GlesTexProgram>,
    /// Set once a compile has been tried and failed, so the warning is logged
    /// once rather than at sixty or two hundred and sixty hertz.
    rounded_failed: bool,
    /// Set once an effect this renderer cannot run has been named, for the
    /// same reason and on the same terms. See [`Programs::refuse`].
    refused: bool,
}

impl Programs {
    /// The rounded-corner program, compiling it on first use.
    ///
    /// `None` means the shader did not compile, and the caller draws the
    /// window square rather than not at all: a driver that cannot build this
    /// program should cost someone their rounded corners, not their desktop.
    pub(crate) fn rounded(&mut self, renderer: &mut GlesRenderer) -> Option<&GlesTexProgram> {
        if self.rounded.is_none() && !self.rounded_failed {
            match renderer.compile_custom_texture_shader(
                ROUNDED_CORNERS,
                &[
                    UniformName::new(RADIUS_UNIFORM, UniformType::_1f),
                    // Ours because smithay gives a texture program no `size`.
                    UniformName::new(SIZE_UNIFORM, UniformType::_2f),
                ],
            ) {
                Ok(program) => self.rounded = Some(program),
                Err(err) => {
                    // Latched before the warning and never cleared, because the
                    // thing that failed is a string constant against a driver:
                    // it will fail identically on the next frame and the one
                    // after, and a warning per frame per window is how a log
                    // stops being readable at the moment somebody needs it.
                    self.rounded_failed = true;
                    tracing::warn!(
                        ?err,
                        "the rounded-corner shader did not compile; windows will be drawn square"
                    );
                }
            }
        }
        self.rounded.as_ref()
    }

    /// Say, once, that a style declares an effect this renderer cannot run.
    ///
    /// The out-loud refusal `fragment::Inputs::Backdrop` asks for. A style
    /// declaring one declares it on every frame of every window it is applied
    /// to, so this latches exactly as `rounded_failed` does and for exactly
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

/// One pane's pass, captured and waiting to be placed.
///
/// Built in `render::prepare`, once a frame, and read by `render::elements`,
/// once per output the window is on -- which is why the position is not in
/// here. A window straddling two monitors is drawn at two different places
/// from this one texture.
///
/// Holds a clone of the program rather than reaching for it again at draw
/// time: a `GlesTexProgram` is a handle to the compiled variants, so this is a
/// refcount, and it means the fallible half -- compiling -- has already
/// happened by the time anything is being drawn.
#[derive(Clone, Debug)]
pub(crate) struct Pass {
    texture: GlesTexture,
    /// The texture's own size, which is what the shader measures in.
    size: Size<i32, Physical>,
    /// **Physical** pixels; see [`physical_radius`].
    radius: f32,
    program: GlesTexProgram,
}

impl Pass {
    /// What `render::prepare` captured, and what it takes to draw it.
    ///
    /// `scale` is the monitor the capture was taken at, and is the one number
    /// that turns `effect`'s logical radius into the shader's physical one.
    pub(crate) fn new(
        texture: GlesTexture,
        size: Size<i32, Physical>,
        effect: Effect,
        scale: f64,
        program: GlesTexProgram,
    ) -> Self {
        Self {
            texture,
            size,
            radius: physical_radius(effect, scale),
            program,
        }
    }

    /// This pass, placed at `dst` on one output, at the pane's opacity.
    ///
    /// A fresh [`Id`] every time, exactly as `warp.rs` does, and for the
    /// stronger version of its reason: the capture is cleared and redrawn on
    /// every frame, so the texture behind this element is new pixels every
    /// frame and full damage is the truth. A stable id with an unchanged
    /// [`CommitCounter`] would report *no* damage after the first frame, and a
    /// window whose client was painting would freeze on screen.
    pub(crate) fn at(&self, dst: Rectangle<i32, Physical>, alpha: f32) -> Rounded {
        Rounded {
            id: Id::new(),
            commit: CommitCounter::default(),
            texture: self.texture.clone(),
            size: self.size,
            dst,
            radius: self.radius,
            program: self.program.clone(),
            alpha,
        }
    }
}

/// A client's own texture, drawn through a fragment program.
///
/// Hand-written rather than a `TextureRenderElement` because none of that
/// type's five constructors takes a program -- checked against
/// `element/texture.rs`, which has `from_texture`,
/// `from_texture_render_buffer`, `from_texture_buffer`,
/// `from_texture_with_damage` and `from_static_texture` and nothing else. The
/// program has to reach the draw some other way, and `warp.rs` is the worked
/// example of an element that carries one.
///
/// **The program is passed to `render_texture_from_to` and not set on the
/// frame**, which is the one decision here worth stating. `GlesFrame` also has
/// `override_default_tex_program`, and an override left set applies to
/// whatever is drawn next -- so a missed `clear_tex_program_override` is a
/// *neighbouring* window with rounded corners it never asked for, appearing
/// only when it happens to be drawn after this one and moving about as windows
/// are raised. `render_texture_from_to`'s last two parameters are
/// `Option<&GlesTexProgram>` and `&[Uniform<'_>]` (`gles/mod.rs:2488`), they
/// take priority over any override (`render_texture`, `gles/mod.rs:2684`), and
/// they are scoped to the one call. There is nothing to clear and therefore
/// nothing to forget to clear.
///
/// **Not covered by any test in this file, and not for want of trying.**
/// `offscreen::Scratch` is generic over what it keeps so its policy can be
/// driven without a GPU; the same trick does not work here, because a
/// `GlesTexProgram` is as unconstructable without a context as a `GlesTexture`
/// is and this element holds one. Everything on it is seen for the first time
/// by Task 6, on a screen.
#[derive(Clone, Debug)]
pub(crate) struct Rounded {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    /// The texture's own size in physical pixels, for `tex_size`. Held rather
    /// than asked of the texture, so the placement does not need one.
    size: Size<i32, Physical>,
    /// Where the client goes on this output, which is not the texture's size:
    /// a window being animated is drawn smaller than it was captured.
    dst: Rectangle<i32, Physical>,
    radius: f32,
    program: GlesTexProgram,
    alpha: f32,
}

impl Element for Rounded {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        // The whole capture. It was created at exactly these pixels -- see
        // `offscreen::capture_client` -- so this is exact rather than rounded,
        // and stating anything else samples outside it or crops a corner off.
        Rectangle::from_size((f64::from(self.size.w), f64::from(self.size.h)).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst
    }

    /// None, for now.
    ///
    /// **Task 5 of this plan is what replaces this**, and it is not a
    /// formality: the client used to be an opaque rectangle, and the damage
    /// tracker skipped drawing whatever was behind it. A rounded client is not
    /// opaque at its corners, so claiming the whole rectangle leaves the
    /// wallpaper undrawn in four little squares. Claiming nothing is merely
    /// slower, which is the right way round to be wrong meanwhile.
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

impl RenderElement<GlesRenderer> for Rounded {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if damage.is_empty() {
            return Ok(());
        }
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            Some(&self.program),
            &[
                Uniform::new(RADIUS_UNIFORM, self.radius),
                // Both physical, which is the pair the shader's `v_coords *
                // tex_size` arithmetic is written against. If this never
                // arrives the uniform stays 0, the shader's clamp makes `r` 0
                // and `away` 0, and the window renders at exactly 50% alpha
                // everywhere -- recognise that rather than hunting a blend bug.
                Uniform::new(SIZE_UNIFORM, (self.size.w as f32, self.size.h as f32)),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // Never a scanout candidate. A plane shows a buffer as it is, and the
        // whole point of this element is that the buffer is not what reaches
        // the screen -- the corners are cut on the way.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The question the renderer asks every pane, every frame. It has to be
    /// cheap and it has to answer `None` for the overwhelmingly common case,
    /// or the ordinary window stops being ordinary.
    #[test]
    fn a_pane_with_no_effects_needs_no_pass() {
        assert_eq!(needs_pass(&[]), None);
    }

    #[test]
    fn an_effect_reading_self_needs_a_pass() {
        let rounded = Effect::rounded(10.0);
        assert_eq!(needs_pass(&[rounded]), Some(rounded));
    }

    /// A zero radius reaches here only if `style::load` let it through, and
    /// `needs_pass` refusing it too is deliberate belt and braces: the cost of
    /// being wrong is every window on the machine rendering offscreen.
    #[test]
    fn a_none_effect_needs_no_pass() {
        assert_eq!(needs_pass(&[Effect::rounded(0.0)]), None);
    }

    /// The first effect wins, and it is the first one that *needs a pass* --
    /// not the first one in the list.
    ///
    /// Without this, `a_none_effect_needs_no_pass` is satisfied by a
    /// `needs_pass` that stops at the first element and reports whatever it
    /// finds there, because a one-element list cannot tell "skipped it" from
    /// "stopped at it". A style that declared a none-effect and a real one
    /// would then round nothing.
    #[test]
    fn a_none_effect_does_not_hide_the_one_behind_it() {
        let rounded = Effect::rounded(8.0);
        assert_eq!(needs_pass(&[Effect::rounded(0.0), rounded]), Some(rounded));
    }

    /// And of two that both want one, the FIRST wins -- which the doc on
    /// `needs_pass` claims and nothing pinned.
    ///
    /// `.rev().find(..)` -- last-effect-wins -- passes every other test in this
    /// module, so without this the doc was a promise the code was free to
    /// break. It cannot be reached from `style::load` today, which pushes at
    /// most one effect; it is pinned because the doc says it, and a claim
    /// nothing can violate is the defect this plan has already removed twice.
    #[test]
    fn of_two_effects_that_both_want_a_pass_the_first_wins() {
        let first = Effect::rounded(4.0);
        let second = Effect::rounded(12.0);
        assert_eq!(needs_pass(&[first, second]), Some(first));
    }

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
        let rounded = Effect::rounded(12.0);
        assert!((physical_radius(rounded, 2.0) - 24.0).abs() < f32::EPSILON);
        assert!((physical_radius(rounded, 1.5) - 18.0).abs() < f32::EPSILON);
        assert!((physical_radius(rounded, 1.0) - 12.0).abs() < f32::EPSILON);
    }

    /// What this renderer can run at all, which is a different question from
    /// whether it needs a pass -- [`Inputs::Nothing`] answers yes here and no
    /// to `needs_pass`, and an implementation that conflated the two would
    /// refuse every ordinary element.
    #[test]
    fn an_effect_reading_a_backdrop_cannot_be_run() {
        assert!(!runnable(Inputs::Backdrop));
        assert!(runnable(Inputs::SelfTexture));
        assert!(
            runnable(Inputs::Nothing),
            "an effect that reads nothing is an ordinary element, not a refusal"
        );
    }

    /// Nothing in this build can construct an effect that reads a backdrop --
    /// `Effect` has one variant and it reads `SelfTexture` -- so this is the
    /// only half of [`refused`] a test can reach, and it is asserted because
    /// the expensive way to be wrong is the other direction: a `refused` that
    /// answered `Some` for the rounded corners a styled window declares would
    /// refuse the feature this plan exists to add.
    #[test]
    fn the_effects_this_build_can_make_are_not_refused() {
        assert_eq!(refused(&[]), None);
        assert_eq!(refused(&[Effect::rounded(12.0)]), None);
        assert_eq!(refused(&[Effect::rounded(0.0)]), None);
    }

    /// Said once, not at the refresh rate.
    ///
    /// A style declaring an unrunnable effect declares it on every frame of
    /// every window it is applied to. Without the latch this is a warning per
    /// window per frame, which is how a log stops being readable at the moment
    /// somebody needs it -- the argument `rounded_failed` already makes, and
    /// the reason the return value exists at all: there is no log subscriber
    /// in a unit test, so the latch is the only observable part.
    #[test]
    fn an_effect_that_cannot_be_run_is_named_once_and_not_every_frame() {
        let mut programs = Programs::default();
        let effect = Effect::rounded(12.0);
        assert!(programs.refuse(effect), "the first refusal says so");
        assert!(!programs.refuse(effect), "and the second says nothing");
        assert!(
            !programs.refuse(Effect::rounded(4.0)),
            "nor a different one"
        );
    }
}
