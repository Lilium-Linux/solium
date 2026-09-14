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
//! per-frame allocation by the number of windows that have one -- and where a
//! warp meant "animating", and so bounded and brief, a `client.radius` is
//! permanent. The multiplier is the number of *visible* styled windows, every
//! frame, for the life of the session. `render::prepare` culls panes no
//! monitor shows for that reason and not as an optimisation.
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
    utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size, Transform},
};
use solium_effects::fragment::{
    Corners, Effect, Inputs, RADIUS_UNIFORM, ROUNDED_CORNERS, SIZE_UNIFORM,
};

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
/// Only reachable with a non-empty effect list. That is not the same as "never
/// on an unstyled machine" any more, and the weaker claim is the true one:
/// `style::load` pushes nothing for an absent or zero `client.radius`, which is
/// thirteen of the fourteen shipped bundles -- but `panes/rounded/` declares
/// `client.radius: 14`, so a session using it walks a one-element list here on
/// the branch below.
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

/// The declared radii, in the **physical** pixels the shader measures in.
///
/// **This is the seam `fragment::RADIUS_UNIFORM` names, and it is the whole of
/// it.** [`Effect::radii`] is logical, because a style writes `radius: 12`
/// into a `Pane.qml` and cannot know which monitor the window will land on;
/// the shader multiplies a normalised coordinate by `tex_size` and measures in
/// the texture's own pixels throughout, and the texture was captured at the
/// monitor's scale.
///
/// The two numbers are equal at scale 1, which is why getting this wrong is
/// perfect on the machine it was written on and wrong on every HiDPI one --
/// and wrong *differently* on each screen of a desk with two scales, since
/// `scale` here is the capturing monitor's and not a constant.
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

/// The largest rectangle certainly inside a rounded rect.
///
/// **The two errors here are not the same size, and the asymmetry decides
/// every rounding in this function.** A region SMALLER than the truth costs
/// drawing: something is painted that did not need to be. A region LARGER than
/// the truth costs correctness: the renderer skips painting what is behind it,
/// and what shows through instead is whatever the last frame left in the
/// buffer -- and, because smithay draws a region an element calls opaque with
/// blending disabled (`gles/mod.rs:2585`), the element's own transparency
/// stops working there too. So every choice below is the small one.
///
/// `sides` is `(top, right, bottom, left)`, **each side inset on its own** --
/// the tuple [`Corners::max_of_side`] returns, in the order it returns it.
/// Each side is measured by the larger of the two corners that touch it,
/// because a side gives up rows only to a corner that is really cut: a
/// square-topped, round-bottomed window that inset all four by one number
/// would throw away twelve rows of a top nothing cuts. That costs drawing and
/// never correctness, which is why it went unnoticed while one radius was the
/// only shape there was -- but it is a cost paid for nothing.
///
/// **The order is the one thing here that fails silently.** Four `f64`s in a
/// tuple type-check in any arrangement, and transposing `right` with `left`
/// draws perfectly on every symmetric window, which is every window a test
/// writes by accident. `a_side_with_no_cut_corner_is_not_inset` is the case
/// that separates them.
///
/// Not the tightest region possible -- the tightest is a cross, since only the
/// four corner squares have anything cut out of them -- but the cross is three
/// rectangles where this is one, and this is certainly inside.
/// `ROUNDED_CORNERS` picks a radius per quadrant, folds the coordinate into
/// one corner and measures `abs(p) - (half - r)`, which is `<= 0` on both axes
/// exactly when the point is at least that quadrant's `r` from both of the
/// edges it touches; the shader then reports `-r` for it, uncut. Insetting a
/// side by the larger of its two corners clears both of them.
///
/// `ceil`, because a physical radius is a logical one times an output scale
/// and is rarely whole: 13 logical at 1.25 is 16.25, and rounding that down
/// claims a quarter-pixel column that the shader cut.
///
/// Saturating throughout, because `Effect::rounded(Corners::all(1e30))` is
/// accepted upstream -- `is_none_effect` refuses only zero, negatives and
/// NaN -- and `radius as i32` saturates at `i32::MAX`, where adding two of
/// them is a debug panic in the middle of a frame.
#[expect(
    clippy::cast_possible_truncation,
    reason = "clamped to i32's range on the line above the cast"
)]
pub(crate) fn opaque_inside(
    rect: Rectangle<i32, Physical>,
    sides: (f64, f64, f64, f64),
) -> Rectangle<i32, Physical> {
    let whole = |side: f64| side.ceil().clamp(0.0, f64::from(i32::MAX)) as i32;
    let (top, right, bottom, left) = sides;
    let (top, right, bottom, left) = (whole(top), whole(right), whole(bottom), whole(left));
    let w = rect
        .size
        .w
        .saturating_sub(left.saturating_add(right))
        .max(0);
    let h = rect
        .size
        .h
        .saturating_sub(top.saturating_add(bottom))
        .max(0);
    Rectangle::new(
        (
            rect.loc.x.saturating_add(left),
            rect.loc.y.saturating_add(top),
        )
            .into(),
        (w, h).into(),
    )
}

/// One element's opaque regions in the space it was drawn into.
///
/// An element states its opaque regions **relative to itself** and its geometry
/// relative to what it was drawn into, so the two are summed. It is the same
/// sum smithay's damage tracker does (`damage/mod.rs:530,580`) and the same one
/// [`opaque_of`] exists to *avoid* doing twice -- which is why it is a named
/// function with a test of its own rather than a closure at its one call site.
///
/// Getting it wrong here is safe, unlike getting it wrong there: a region put
/// in the wrong place fails to cover the capture, [`covers`] answers false, and
/// the window claims nothing. That is why this is low stakes, not why it is
/// fine.
pub(crate) fn placed(
    loc: Point<i32, Physical>,
    regions: impl IntoIterator<Item = Rectangle<i32, Physical>>,
) -> impl Iterator<Item = Rectangle<i32, Physical>> {
    regions.into_iter().map(move |mut region| {
        region.loc += loc;
        region
    })
}

/// Whether `regions` leave no part of a `size`-sized rectangle uncovered.
///
/// Asked of a capture once, in `offscreen::capture_client`, with the opaque
/// regions the client's own surfaces declared. The capture is cleared to
/// transparent and the client draws into it, so the only thing that makes any
/// of it opaque is the client saying so -- and a translucent client is
/// ordinary, not exotic. See [`opaque_of`], which is what the answer gates.
///
/// The whole capture rather than only the part [`opaque_inside`] would claim,
/// which is stricter than needed and deliberately so: it is one question
/// instead of one per placement, and it means the sampler cannot reach a
/// transparent texel from an opaque fragment however the texture is filtered.
///
/// **`!size.is_empty()` is belt and braces, and is recorded as such because
/// removing it changes no test.** Smithay already answers `false` for a
/// zero-sized capture, but only as a consequence of `intersection` returning
/// `None` for a zero-area overlap (`geometry.rs:1364`): an empty rect can
/// never be subtracted away, so it stays in the remainder. That is a detail of
/// how `subtract_rects` treats degenerate input rather than a promise about
/// this question, and the direction it would fail in if it changed is the
/// expensive one -- a capture with no pixels called opaque everywhere.
pub(crate) fn covers(
    size: Size<i32, Physical>,
    regions: impl IntoIterator<Item = Rectangle<i32, Physical>>,
) -> bool {
    !size.is_empty()
        && Rectangle::from_size(size)
            .subtract_rects(regions)
            .is_empty()
}

/// Whether every corner is cut by at least half a physical pixel, which is
/// what it takes for **any** of this window to be opaque.
///
/// `ROUNDED_CORNERS` omits the interior term of the rounded-box field: it
/// reports `-r` at *every* point inside the shape rather than the real
/// distance to the edge, and that answer goes through
/// `smoothstep(-0.5, 0.5, away)`. So a quadrant whose radius is under half a
/// pixel is not softened at an arc -- it is drawn **uniformly translucent**,
/// all of it, and a zero radius lands exactly on the smoothstep's midpoint and
/// draws that quarter of the window at 50%. `dev/wirecheck` says the same
/// thing from the other end: an unset `corner_radius` clamps `r` to 0 and
/// "the whole texture comes back at alpha 127".
///
/// **Per corner, and that is the change a square corner forces.** With one
/// radius this could only be reached by a style asking for a sub-pixel
/// rounding, which nothing sensible does. With four it is the ordinary
/// square-topped window -- `radiusTopLeft: 0` -- and claiming its top rows
/// opaque would hand a 50%-alpha quadrant to a draw with blending disabled
/// (`gles/mod.rs:2585`), which paints over the wallpaper rather than blending
/// with it.
///
/// **This is conservative against a shader defect and not against geometry.**
/// A square corner *should* be opaque up to its own edge; it is the missing
/// `min(max(p.x, p.y), 0.0)` term that makes it translucent instead. Adding
/// that term is a change to `fragment.rs` -- out of this file, and it would
/// leave every other fragment's answer untouched, since the term is zero
/// wherever either component of `p` is positive. Until it is added, this
/// refuses the whole window: a window drawn at half alpha over the wallpaper
/// merely looks wrong, and one drawn at half alpha with blending off corrupts
/// what is behind it.
///
/// NaN answers `false` here -- `NaN >= 0.5` is false -- and is named rather
/// than left to fall out of a negation, because it compares false against
/// every bound and would otherwise reach [`opaque_inside`] and inset by zero.
fn every_corner_is_cut(radii: Corners) -> bool {
    [
        radii.top_left,
        radii.top_right,
        radii.bottom_left,
        radii.bottom_right,
    ]
    .into_iter()
    .all(|radius| radius >= 0.5)
}

/// The part of a rounded capture that is certainly opaque on screen, **as a
/// rectangle relative to the element**, or `None` if none of it is.
///
/// Relative to the element because that is the frame smithay documents
/// `Element::opaque_regions` in -- "the opaque regions of the element relative
/// to the element" -- and the damage tracker adds the element's own location
/// back on (`damage/mod.rs:530,580`). A claim built from the drawn rect whole
/// is therefore offset twice and lands at `2 * dst.loc`: a patch of desktop
/// with no window on it, left unpainted. That is this task's own failure
/// pointed somewhere arbitrary, so the origin is dropped here rather than at
/// the call site.
///
/// Three things make it `None`, and each is a way the window is opaque
/// *nowhere* rather than merely not at its corners:
///
/// * the client did not cover its own capture (see [`covers`]);
/// * the pane is mid-fade. The damage tracker reads `opaque_regions` and
///   `alpha` separately and never multiplies one into the other, so an element
///   has to do it -- smithay's own `TextureRenderElement` returns nothing below
///   1.0 (`element/texture.rs:647`) for exactly this reason;
/// * **any one** of the four radii is under half a physical pixel -- see
///   [`every_corner_is_cut`], which is where the whole of that argument is
///   written down. One corner is enough, because the shader picks its radius
///   per quadrant and a quadrant is a quarter of the window.
///
/// `widen` is the last of it, and it is the one a 1:1 screen cannot feel. The
/// radius is in the **texture's** pixels, and the texture is not always drawn
/// at its own size: an animated window is captured once at its real size and
/// stretched onto whatever rect it has this frame, mask and all. Drawn
/// smaller, the corner shrinks and a smaller inset would do; drawn *larger* --
/// which a script setting a rect bigger than the window's geometry does -- the
/// corner grows, and an inset of `radius` screen pixels would be too small,
/// which is the expensive direction. [`crate::render::ratio`] is the same
/// number `elements` scales the ordinary path by, asked here rather than
/// defined a second time, and it answers 1.0 for the degenerate sizes.
///
/// It widens all four the same, because it is the *texture* that is being
/// stretched: one capture, one rect, one ratio per axis, and the larger of the
/// two taken for every side. A per-side ratio would be a second answer to a
/// question the texture has already answered.
pub(crate) fn opaque_of(
    dst: Rectangle<i32, Physical>,
    texture: Size<i32, Physical>,
    radii: Corners,
    alpha: f32,
    capture_opaque: bool,
) -> Option<Rectangle<i32, Physical>> {
    if !capture_opaque || alpha < 1.0 || !every_corner_is_cut(radii) {
        return None;
    }
    let widen = crate::render::ratio(f64::from(dst.size.w), texture.w)
        .max(crate::render::ratio(f64::from(dst.size.h), texture.h));
    // `(top, right, bottom, left)`, in that order into `opaque_inside`, which
    // takes it in that order. Destructured and rebuilt by name rather than
    // mapped over a tuple, so a transposition here has to be written out on
    // purpose.
    let (top, right, bottom, left) = radii.max_of_side();
    let region = opaque_inside(
        Rectangle::from_size(dst.size),
        (top * widen, right * widen, bottom * widen, left * widen),
    );
    (!region.is_empty()).then_some(region)
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
                    UniformName::new(RADIUS_UNIFORM, UniformType::_4f),
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
    /// **Physical** pixels, all four; see [`physical_radii`].
    radii: Corners,
    /// Whether the client covered the whole capture with opaque regions of its
    /// own, which is the only thing that makes any of this texture opaque: it
    /// was cleared to transparent before the client drew into it. See
    /// [`covers`], which answers it, and [`opaque_of`], which reads it.
    opaque: bool,
    program: GlesTexProgram,
}

impl Pass {
    /// What `render::prepare` captured, and what it takes to draw it.
    ///
    /// `scale` is the monitor the capture was taken at, and is the one number
    /// that turns `effect`'s logical radii into the shader's physical ones.
    pub(crate) fn new(
        texture: GlesTexture,
        size: Size<i32, Physical>,
        effect: Effect,
        scale: f64,
        opaque: bool,
        program: GlesTexProgram,
    ) -> Self {
        Self {
            texture,
            size,
            radii: physical_radii(effect, scale),
            opaque,
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
            radii: self.radii,
            opaque: self.opaque,
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
/// **Almost none of it is covered by a test, and not for want of trying.**
/// `offscreen::Scratch` is generic over what it keeps so its policy can be
/// driven without a GPU; the same trick does not work here, because a
/// `GlesTexProgram` is as unconstructable without a context as a `GlesTexture`
/// is and this element holds one. The exception is `opaque_regions`, which is
/// the one answer here whose cost of being wrong is a corrupt screen rather
/// than a wrong picture: all of it lives in [`opaque_of`], which takes numbers
/// and is tested against the shader's own distance field. The rest -- `src`,
/// `geometry`, `alpha`, `draw` -- is seen for the first time by Task 6, on a
/// screen.
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
    /// **Physical** pixels, in `Corners`' own field order, which is the order
    /// the shader indexes `corner_radius` by. See [`physical_radii`].
    radii: Corners,
    /// See [`Pass::opaque`], whose copy this is.
    opaque: bool,
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

    /// The drawn rect, inset on each side by the larger of the two corners
    /// that touch it -- or nothing at all.
    ///
    /// **This is the point of the whole plan, and the reason rounded corners
    /// were chosen as the first effect rather than something prettier.** A
    /// square client is opaque everywhere, and the damage tracker uses that to
    /// skip drawing whatever is behind it. A rounded one is not opaque at four
    /// places, so an element that kept claiming its whole rectangle would leave
    /// the wallpaper undrawn in four little squares -- and what is there
    /// instead is whatever the last frame left in the buffer, which reads as
    /// four smears following the window around. Opacity becomes a property the
    /// node declares and the culling reads, rather than a global assumption
    /// that quietly stopped being true.
    ///
    /// [`opaque_of`] is all of it, including the three ways this answers
    /// nothing at all; it is a free function because nothing in a test can
    /// construct a `GlesTexture` or a `GlesTexProgram`, and this decision is
    /// too expensive to be wrong to leave where only a screen can check it.
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        opaque_of(self.dst, self.size, self.radii, self.alpha, self.opaque)
            .map_or_else(OpaqueRegions::default, |region| {
                OpaqueRegions::from_slice(&[region])
            })
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Rounded {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a corner radius is tens of pixels; f32 is what the uniform takes"
    )]
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
                // **`(tl, tr, bl, br)`, which is `Corners`' own field order and
                // is the order the shader indexes.** `corner_radius.x` is read
                // where `v_coords` is in the top-left quadrant, `.y` top-right,
                // `.z` bottom-left, `.w` bottom-right -- so this tuple and
                // `Corners`' declaration have to stay in step, and nothing but
                // a screen would notice if they stopped: a transposed pair
                // draws a correct-looking window with its corners swapped, and
                // on the overwhelmingly common window where all four agree it
                // draws nothing wrong at all.
                //
                // A 4-tuple and not a bare `f32`, which would register as
                // `_1f` against a `vec4` location and mismatch the
                // `UniformType::_4f` this program was compiled with in
                // `Programs::rounded`, leaving every fragment unset -- not a
                // theoretical risk, `dev/wirecheck` hit exactly this.
                Uniform::new(
                    RADIUS_UNIFORM,
                    (
                        self.radii.top_left as f32,
                        self.radii.top_right as f32,
                        self.radii.bottom_left as f32,
                        self.radii.bottom_right as f32,
                    ),
                ),
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
    // `Corners` arrives with everything else: `pass` imports it at the top of
    // the file now that `Rounded` holds one, so a second `use` here would be
    // the same name reached two ways.
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
        let rounded = Effect::rounded(Corners::all(10.0));
        assert_eq!(needs_pass(&[rounded]), Some(rounded));
    }

    /// A zero radius reaches here only if `style::load` let it through, and
    /// `needs_pass` refusing it too is deliberate belt and braces: the cost of
    /// being wrong is every window on the machine rendering offscreen.
    #[test]
    fn a_none_effect_needs_no_pass() {
        assert_eq!(needs_pass(&[Effect::rounded(Corners::all(0.0))]), None);
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
        let rounded = Effect::rounded(Corners::all(8.0));
        assert_eq!(
            needs_pass(&[Effect::rounded(Corners::all(0.0)), rounded]),
            Some(rounded)
        );
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
        let first = Effect::rounded(Corners::all(4.0));
        let second = Effect::rounded(Corners::all(12.0));
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
        assert_eq!(refused(&[Effect::rounded(Corners::all(12.0))]), None);
        assert_eq!(refused(&[Effect::rounded(Corners::all(0.0))]), None);
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
        let effect = Effect::rounded(Corners::all(12.0));
        assert!(programs.refuse(effect), "the first refusal says so");
        assert!(!programs.refuse(effect), "and the second says nothing");
        assert!(
            !programs.refuse(Effect::rounded(Corners::all(4.0))),
            "nor a different one"
        );
    }

    /// The question the whole design rests on, and the reason rounded corners
    /// were chosen as the first effect rather than something prettier.
    ///
    /// A square window is opaque everywhere, and the renderer uses that to
    /// skip drawing whatever is behind it. Round the corners and that stops
    /// being true at four places -- so an element that keeps claiming the
    /// whole rect leaves the wallpaper undrawn under each corner, and what is
    /// there instead is whatever the last frame left, which reads as four
    /// smears that follow the window around.
    ///
    /// Inset on every side by the corner that cuts it: the largest rectangle
    /// that is certainly inside a rounded rect. Not the tightest possible
    /// region -- the tightest is a cross -- but it is right, and a region that
    /// is smaller than the truth only costs drawing, where one larger than the
    /// truth costs correctness.
    ///
    /// All four equal here, which is the case every window had before this
    /// change and the overwhelmingly common one after it. The asymmetric shape
    /// is `a_side_with_no_cut_corner_is_not_inset`, one test down.
    #[test]
    fn a_rounded_rect_is_opaque_only_inside_its_corners() {
        let rect = Rectangle::<i32, Physical>::new((100, 100).into(), (300, 200).into());
        let opaque = opaque_inside(rect, (20.0, 20.0, 20.0, 20.0));
        assert_eq!(opaque.loc.x, 120);
        assert_eq!(opaque.loc.y, 120);
        assert_eq!(opaque.size.w, 260);
        assert_eq!(opaque.size.h, 160);
    }

    /// **A square-topped window keeps its top rows.**
    ///
    /// The one-radius signature this replaced inset all four sides by the same
    /// number, which for the Finder shape -- square on top, cut underneath --
    /// would have thrown away twelve rows of a top that nothing cuts at all.
    /// That costs drawing and never correctness, which is why nothing noticed
    /// while a window had one radius; it is a cost paid for nothing.
    ///
    /// Four different numbers, and every side asserted, because the tuple is
    /// where an ordering mistake is silent: `(0, 20, 20, 12)` transposed into
    /// `(0, 12, 20, 20)` type-checks, draws identically on every symmetric
    /// window, and fails here on `loc.x`.
    #[test]
    fn a_side_with_no_cut_corner_is_not_inset() {
        let rect = Rectangle::<i32, Physical>::new((100, 100).into(), (300, 200).into());
        let inside = opaque_inside(rect, (0.0, 20.0, 20.0, 12.0));
        assert_eq!(inside.loc.y, 100, "nothing is cut along the top");
        assert_eq!(
            inside.loc.x, 112,
            "the left side is inset by its larger corner"
        );
        assert_eq!(inside.size.h, 180, "only the bottom is taken");
        assert_eq!(inside.size.w, 268);
    }

    /// A radius larger than the window is not a negative rectangle.
    #[test]
    fn a_radius_bigger_than_the_window_claims_nothing() {
        let rect = Rectangle::<i32, Physical>::new((0, 0).into(), (30, 30).into());
        let opaque = opaque_inside(rect, (40.0, 40.0, 40.0, 40.0));
        assert_eq!(opaque.size.w, 0);
        assert_eq!(opaque.size.h, 0);
        // The location as well as the size, because the brief's two assertions
        // leave it free: an empty rect is harmless wherever it is, but nothing
        // else here pins the `loc` arithmetic on the degenerate path, and a
        // rect that is empty only by its size is one `max(0)` away from being
        // a claim somewhere arbitrary.
        assert_eq!(opaque.loc.x, 40);
        assert_eq!(opaque.loc.y, 40);
        // And one side alone is enough, which the symmetric case above cannot
        // say: each axis now subtracts a *sum* of two different sides, so a
        // `max(0)` that was reached by both being large is not the same
        // assertion as one reached by either.
        let one_side = opaque_inside(rect, (40.0, 0.0, 0.0, 0.0));
        assert_eq!(one_side.size.h, 0, "a top deeper than the window");
        assert_eq!(one_side.size.w, 30, "and the other axis untouched by it");
    }

    /// Rounded **up**, and the whole test is the direction.
    ///
    /// The two tests above pass unchanged with `floor`, `round`, `trunc` or a
    /// bare `as i32`, because 20.0 and 40.0 are already whole. A physical
    /// radius is not: it is a logical one times an output scale, so 12 at
    /// 1.25 is 15.0 and 13 at 1.25 is 16.25. Rounding that down claims a
    /// quarter-pixel column the shader cut, which is the expensive direction.
    #[test]
    fn a_fractional_radius_insets_by_the_whole_pixel_it_touches() {
        let rect = Rectangle::<i32, Physical>::new((0, 0).into(), (100, 100).into());
        let opaque = opaque_inside(rect, (16.25, 16.25, 16.25, 16.25));
        assert_eq!(opaque.loc.x, 17, "16.25 has to inset 17, not 16");
        assert_eq!(opaque.size.w, 66);
        // Each side rounded on its own and not once for all four: with four
        // sides there is a rounding per side, and one of them applied to the
        // rest would claim a quarter-pixel column the shader cut on whichever
        // sides it was not computed from.
        let mixed = opaque_inside(rect, (16.25, 4.0, 0.25, 8.75));
        assert_eq!(mixed.loc.y, 17);
        assert_eq!(mixed.loc.x, 9, "8.75 has to inset 9");
        assert_eq!(mixed.size.h, 82, "17 off the top and 1 off the bottom");
        assert_eq!(mixed.size.w, 87, "9 off the left and 4 off the right");
    }

    /// `Effect::rounded(Corners::all(1e9))` is accepted upstream --
    /// `is_none_effect` refuses only zero, negatives and NaN -- so an absurd
    /// radius reaches here.
    ///
    /// `radius.ceil() as i32` saturates at `i32::MAX`, and adding two of those
    /// together is an overflow: a debug panic, in the middle of a frame, taking
    /// the compositor with it. Nothing else in this file would notice.
    ///
    /// Two saturated sides on one axis and not one, because the inset per axis
    /// is now `left + right` rather than `inset * 2` -- and `i32::MAX + 0` does
    /// not overflow where `i32::MAX + i32::MAX` does.
    #[test]
    fn an_absurd_radius_does_not_overflow_the_inset() {
        let rect = Rectangle::<i32, Physical>::new((0, 0).into(), (100, 100).into());
        assert!(opaque_inside(rect, (1e30, 1e30, 1e30, 1e30)).is_empty());
        assert!(
            opaque_inside(
                rect,
                (f64::INFINITY, f64::INFINITY, f64::INFINITY, f64::INFINITY)
            )
            .is_empty()
        );
        assert!(
            opaque_inside(rect, (1e30, 0.0, 1e30, 0.0)).is_empty(),
            "one axis saturated at both ends is the sum that overflows"
        );
    }

    /// **Relative to the element, not to the output.** Smithay's `Element`
    /// spells it out -- "the opaque regions of the element relative to the
    /// element" -- and the damage tracker adds the element's own location
    /// back on (`damage/mod.rs:530,580`), exactly as it does for damage.
    ///
    /// So the rect to inset is the drawn rect's *size* at the origin, and a
    /// claim built from `self.dst` whole is offset twice: it lands at
    /// `2 * dst.loc`, somewhere else on the screen entirely, and tells the
    /// renderer to stop painting a patch of desktop that has no window on it.
    /// That is the failure this task exists to prevent, aimed at a random
    /// rectangle instead of at four corners.
    ///
    /// Pinned by moving the window and asserting nothing changes. A version
    /// that returned `opaque_inside(dst, sides)` agrees with every other test
    /// in this module, because all of them place the element at the origin.
    #[test]
    fn the_claim_is_relative_to_the_element_not_to_the_output() {
        let texture = Size::<i32, Physical>::from((300, 200));
        let at_origin = Rectangle::<i32, Physical>::new((0, 0).into(), texture);
        let far_away = Rectangle::<i32, Physical>::new((1920, 1080).into(), texture);
        assert_eq!(
            opaque_of(at_origin, texture, Corners::all(20.0), 1.0, true),
            opaque_of(far_away, texture, Corners::all(20.0), 1.0, true),
            "where the window is on screen cannot change what it claims"
        );
        assert_eq!(
            opaque_of(far_away, texture, Corners::all(20.0), 1.0, true),
            Some(Rectangle::new((20, 20).into(), (260, 160).into())),
        );
    }

    /// **The radius is measured in the texture's pixels, and the texture is
    /// not always drawn at its own size.**
    ///
    /// A window being animated is captured once, at its real size, and drawn
    /// into more or less of the screen; the mask lives in the texture, so the
    /// corner is stretched or squeezed with everything else. An inset of
    /// `radius` screen pixels is therefore only right at 1:1 -- and it is
    /// wrong in the *expensive* direction whenever the window is drawn larger
    /// than it was captured, which a script setting a rect bigger than the
    /// window's geometry does.
    ///
    /// Both halves are asserted because ignoring the magnification entirely
    /// satisfies the shrinking one if it is stated as an inequality: the
    /// doubled case is the one that fails.
    #[test]
    fn the_inset_is_the_radius_as_it_lands_on_screen() {
        let texture = Size::<i32, Physical>::from((400, 400));
        let doubled = Rectangle::<i32, Physical>::new((0, 0).into(), (800, 800).into());
        assert_eq!(
            opaque_of(doubled, texture, Corners::all(20.0), 1.0, true),
            Some(Rectangle::new((40, 40).into(), (720, 720).into())),
            "drawn at twice its size, the corner is twice as big"
        );
        let halved = Rectangle::<i32, Physical>::new((0, 0).into(), (200, 200).into());
        assert_eq!(
            opaque_of(halved, texture, Corners::all(20.0), 1.0, true),
            Some(Rectangle::new((10, 10).into(), (180, 180).into())),
            "and half as big drawn half the size"
        );
    }

    /// **Stretched more on one axis than the other, which is the case that
    /// tells `max` from `min`.**
    ///
    /// Every other magnification here is uniform, and under `sx == sy` the four
    /// wrong implementations -- `min`, `sx` alone, `sy` alone, and the right
    /// one -- are indistinguishable. So this is the only test in the module
    /// that fails when `.max(` becomes `.min(`, and the reason `widen` takes
    /// the larger of the two is written down as a value rather than as prose.
    ///
    /// Reachable: `sol.present(id, {w, h})` reads `w` and `h` separately and
    /// clamps neither (`script.rs`), so `{w = 2 * w, h = h}` is `sx = 2,
    /// sy = 1`. With `min` the inset would be `ceil(r)` where the horizontal
    /// arc needs `ceil(2r)`, and a column `r` wide down each corner would be
    /// claimed opaque -- last-frame garbage, and the window's own output
    /// written with blending off.
    #[test]
    fn a_window_stretched_on_one_axis_insets_by_the_wider_of_the_two() {
        let wide = Size::<i32, Physical>::from((400, 200));
        let stretched = Rectangle::<i32, Physical>::new((0, 0).into(), (800, 200).into());
        assert_eq!(
            opaque_of(stretched, wide, Corners::all(20.0), 1.0, true),
            Some(Rectangle::new((40, 40).into(), (720, 120).into())),
            "the horizontal arc doubled, so 40 is the inset on every side"
        );
        // And the same the other way up. Without it, `widen = sx` -- the
        // horizontal ratio alone -- passes the case above and every other test
        // in this module, and a window stretched vertically would claim a row
        // `r` deep across each corner. Found by mutation, not by reading.
        let tall = Size::<i32, Physical>::from((200, 400));
        let stood_up = Rectangle::<i32, Physical>::new((0, 0).into(), (200, 800).into());
        assert_eq!(
            opaque_of(stood_up, tall, Corners::all(20.0), 1.0, true),
            Some(Rectangle::new((40, 40).into(), (120, 720).into())),
            "the vertical arc doubled, and the inset has to follow that axis too"
        );
    }

    /// **Four radii land on four sides, in the right places.**
    ///
    /// The whole of this task between `Corners` and a rectangle, and the only
    /// test in the module where a transposition can show. Every other
    /// `opaque_of` case here is `Corners::all`, under which all six possible
    /// swaps of the four fields, and both possible orderings of the tuple, give
    /// the identical answer.
    ///
    /// The numbers are chosen so each side takes a *different* corner, which is
    /// what makes `max_of_side` visible rather than assumed: the left side is
    /// insetting by `bottom_left` because 30 beats `top_left`'s 6, and the top
    /// by `top_right` because 20 beats the same 6. A `max_of_side` that took
    /// the smaller of the two, or the first of them, fails on both.
    ///
    /// **Every corner is at least half a pixel on purpose.** Put a 0 in here
    /// and the answer is `None` for a reason that has nothing to do with
    /// sides -- see [`every_corner_is_cut`] -- and this test would be asserting
    /// that guard rather than the arithmetic.
    #[test]
    fn each_side_is_inset_by_the_larger_of_the_two_corners_touching_it() {
        let texture = Size::<i32, Physical>::from((300, 200));
        let dst = Rectangle::<i32, Physical>::new((0, 0).into(), texture);
        let radii = Corners {
            top_left: 6.0,
            top_right: 20.0,
            bottom_left: 30.0,
            bottom_right: 12.0,
        };
        // top = max(6, 20) = 20; right = max(20, 12) = 20;
        // bottom = max(30, 12) = 30; left = max(6, 30) = 30.
        assert_eq!(
            opaque_of(dst, texture, radii, 1.0, true),
            Some(Rectangle::new((30, 20).into(), (250, 150).into())),
            "the left side is cut by its bottom corner and the top by its \
             right one, so neither is the corner sharing its name"
        );
    }

    /// A half-faded window is opaque nowhere, and the damage tracker will not
    /// work that out on its own.
    ///
    /// It reads `Element::opaque_regions` and `Element::alpha` separately and
    /// never multiplies one into the other -- checked in `damage/mod.rs`,
    /// where `alpha` is used only to decide whether the element *moved*
    /// (`instance_matches`, line 631). Smithay's own `TextureRenderElement`
    /// therefore returns nothing at all below 1.0 (`element/texture.rs:647`),
    /// and this has to do the same or a window fading in leaves the wallpaper
    /// behind it undrawn across its whole middle rather than at four corners.
    ///
    /// 0.999 as well as 0.5, because `alpha <= 0.5` and `alpha == 0.0` are
    /// both wrong implementations that a single half-opacity case accepts.
    #[test]
    fn a_fading_window_is_opaque_nowhere() {
        let texture = Size::<i32, Physical>::from((300, 200));
        let dst = Rectangle::<i32, Physical>::new((0, 0).into(), texture);
        assert_eq!(opaque_of(dst, texture, Corners::all(20.0), 0.5, true), None);
        assert_eq!(
            opaque_of(dst, texture, Corners::all(20.0), 0.999, true),
            None
        );
        assert!(
            opaque_of(dst, texture, Corners::all(20.0), 1.0, true).is_some(),
            "and a window that is not fading still claims its middle"
        );
    }

    /// **A rounded window is no more opaque than the same window was square.**
    ///
    /// The capture is cleared to transparent and the client's surfaces are
    /// drawn into it, so what is in the texture is whatever the client put
    /// there: a terminal at 80% background, a GTK app that rounds its own
    /// corners, a client that has not painted its whole geometry. Claiming
    /// the middle of *that* opaque is the same defect as claiming the corners
    /// -- and worse here than at the corners, because smithay draws a region
    /// an element calls opaque with blending **disabled**
    /// (`gles/mod.rs:2585`), so a translucent client would not merely smear:
    /// it would stop being translucent.
    ///
    /// So the capture is asked, once, whether the client covered it with
    /// opaque regions of its own, and nothing is claimed unless it did. That
    /// makes this element's claim a subset of what the client's own surfaces
    /// claimed on the ordinary path, which is the property worth having.
    #[test]
    fn a_capture_the_client_left_translucent_is_opaque_nowhere() {
        let texture = Size::<i32, Physical>::from((300, 200));
        let dst = Rectangle::<i32, Physical>::new((0, 0).into(), texture);
        assert_eq!(
            opaque_of(dst, texture, Corners::all(20.0), 1.0, false),
            None
        );
        assert!(opaque_of(dst, texture, Corners::all(20.0), 1.0, true).is_some());
    }

    /// Below half a physical pixel of radius, *nothing* is opaque -- and that
    /// is a property of the shader rather than of the geometry.
    ///
    /// `ROUNDED_CORNERS` computes `away = length(max(p, 0.0)) - r`, which
    /// omits the interior term of the exact rounded-box field: every fragment
    /// inside the shape reports `-r` and not its real distance to the edge. It
    /// is then fed to `smoothstep(-0.5, 0.5, away)`, so a window with `r` under
    /// a half pixel comes out uniformly *translucent everywhere*, not merely
    /// softened at four arcs. Claiming any of it opaque would draw it with
    /// blending off and paint over what is behind it.
    ///
    /// `0.0` alone is not enough: refusing only a zero radius is a wrong
    /// implementation that this catches and that one would not.
    #[test]
    fn a_radius_under_half_a_pixel_is_opaque_nowhere() {
        let texture = Size::<i32, Physical>::from((300, 200));
        let dst = Rectangle::<i32, Physical>::new((0, 0).into(), texture);
        assert_eq!(opaque_of(dst, texture, Corners::all(0.4), 1.0, true), None);
        assert_eq!(
            opaque_of(dst, texture, Corners::all(f64::NAN), 1.0, true),
            None,
            "NaN compares false against every bound and must not fall through \
             to an inset of zero, which would claim the whole rectangle"
        );
        // The bound is exact rather than approximate -- `smoothstep(-0.5, 0.5,
        // away)` is 0 at `away == -0.5` and not merely close to it -- so both
        // sides of it are asserted rather than just the far side. 0.4 alone
        // leaves every threshold in (0.4, 0.5] passing, and each of those is a
        // window claimed opaque that the shader has faded.
        assert_eq!(opaque_of(dst, texture, Corners::all(0.49), 1.0, true), None);
        assert!(opaque_of(dst, texture, Corners::all(0.5), 1.0, true).is_some());
    }

    /// **One square corner is enough**, and it is the case per-corner radii
    /// make ordinary rather than exotic.
    ///
    /// The shader picks its radius per quadrant, and the field it evaluates has
    /// no interior term -- so the quadrant whose radius is 0 is not a sharp
    /// corner, it is a quarter of the window drawn at exactly 50% alpha. See
    /// [`every_corner_is_cut`], which is where that argument lives.
    ///
    /// **This is the one place this file gives up more than the geometry says
    /// it must**, and it is deliberate: claiming that quadrant would hand
    /// 50%-alpha pixels to a draw with blending disabled, which paints over the
    /// wallpaper instead of blending with it. The moment `fragment.rs` gains
    /// the `min(max(p.x, p.y), 0.0)` term, a square corner becomes genuinely
    /// opaque to its own edge and this bound can drop to `>= 0.0` -- at which
    /// point `a_side_with_no_cut_corner_is_not_inset` stops being an
    /// arithmetic test and starts describing a window on a screen.
    ///
    /// Each of the four in turn, because a guard written against one field --
    /// or against `max_of_side`, or against `largest` -- passes every other
    /// assertion in this module. `largest()` in particular is the wrong
    /// question exactly backwards: `Corners { top_left: 0.0, ..all(20.0) }` has
    /// a largest of 20 and a quarter of the window at half alpha.
    #[test]
    fn a_single_square_corner_is_opaque_nowhere() {
        let texture = Size::<i32, Physical>::from((300, 200));
        let dst = Rectangle::<i32, Physical>::new((0, 0).into(), texture);
        for square in [
            Corners {
                top_left: 0.0,
                ..Corners::all(20.0)
            },
            Corners {
                top_right: 0.0,
                ..Corners::all(20.0)
            },
            Corners {
                bottom_left: 0.0,
                ..Corners::all(20.0)
            },
            Corners {
                bottom_right: 0.0,
                ..Corners::all(20.0)
            },
        ] {
            assert_eq!(
                opaque_of(dst, texture, square, 1.0, true),
                None,
                "{square:?} leaves one quadrant at 50% alpha, and a region \
                 claimed opaque is drawn with blending disabled"
            );
        }
        assert!(
            opaque_of(dst, texture, Corners::all(20.0), 1.0, true).is_some(),
            "and four cut corners still claim the middle -- without this the \
             guard could refuse everything and pass"
        );
    }

    /// The sum `capture_client` makes before it asks [`covers`] anything.
    ///
    /// Stated as a case that is covered **only** if the shift happens and only
    /// if it is an addition: the region says (0, 0) and the element sits at
    /// (0, 50), so leaving the sum out, subtracting instead of adding, or
    /// shifting the size rather than the location each leaves the bottom half
    /// of the capture uncovered.
    #[test]
    fn an_elements_opaque_regions_are_moved_to_where_it_was_drawn() {
        let size = Size::<i32, Physical>::from((100, 100));
        let half = Rectangle::<i32, Physical>::new((0, 0).into(), (100, 50).into());
        assert_eq!(
            placed(Point::from((10, 20)), [half]).collect::<Vec<_>>(),
            vec![Rectangle::new((10, 20).into(), (100, 50).into())],
            "the location moves and the size does not"
        );
        assert!(
            covers(
                size,
                placed(Point::from((0, 0)), [half]).chain(placed(Point::from((0, 50)), [half]))
            ),
            "a top half and a bottom half cover the capture once each is put \
             where its element was drawn"
        );
        assert!(
            !covers(size, [half, half]),
            "and do not if the second is left where it says it is"
        );
    }

    /// The lines of `ROUNDED_CORNERS` that [`away`] is transcribed from, and
    /// the threshold it is compared against.
    ///
    /// **Without this the transcription is a second opinion, which is the one
    /// thing it claims not to be.** Editing the distance field in
    /// `fragment.rs` would otherwise leave every assertion below green while
    /// the shader cut a different shape from the one this file insets against
    /// -- and the failure would be invisible until a screen. `fragment.rs` has
    /// its own copy of this idea and pins every line below somewhere in its
    /// own tests too -- the five that were here before Task 1, plus the three
    /// that pick `corner_radius`'s quadrant, which is a second opinion by a
    /// different mechanism (whole-line `has_line`, a whitelist walk) rather
    /// than a reason to trust this one less.
    ///
    /// Whole lines rather than `contains`, for `fragment.rs`'s reason: every
    /// name in this shader is a substring of something else legitimately in it.
    ///
    /// **The three `picked` lines are also the only written-down statement of
    /// which `vec4` component is which corner**, and two things in this file
    /// depend on that answer: [`away`]'s `match`, and the tuple
    /// `Rounded::draw` hands to `Uniform::new`. Read off the lines below,
    /// `x < 0.5 && y < 0.5` -- the top-left quadrant -- takes `corner_radius.x`,
    /// so the packing is `(top_left, top_right, bottom_left, bottom_right)`,
    /// which is `Corners`' own field order. That is why `draw` names each field
    /// rather than spreading a struct: the two orders agreeing is a fact about
    /// these lines, not about the type.
    const FIELD: [&str; 8] = [
        "vec2 half_size = tex_size * 0.5;",
        "float picked = (v_coords.x < 0.5)",
        "? ((v_coords.y < 0.5) ? corner_radius.x : corner_radius.z)",
        ": ((v_coords.y < 0.5) ? corner_radius.y : corner_radius.w);",
        "float r = min(picked, min(half_size.x, half_size.y));",
        "vec2 p = abs(v_coords * tex_size - half_size) - (half_size - vec2(r));",
        "float away = length(max(p, 0.0)) - r;",
        "gl_FragColor = colour * (1.0 - smoothstep(-0.5, 0.5, away));",
    ];

    /// `ROUNDED_CORNERS`'s distance field, evaluated in Rust.
    ///
    /// Transcribed from the lines of `fragment.rs` that compute it -- named in
    /// [`FIELD`] and asserted still to be there, so that the claim this file
    /// makes is checked against the program that actually cuts the corners
    /// rather than against a second opinion about geometry. `point` is
    /// `v_coords * tex_size`: the fragment's position in the texture's own
    /// pixels.
    ///
    /// **Four radii, picked per quadrant, exactly where the shader picks.**
    /// This used to take one `f64` and was faithful only while `Rounded::draw`
    /// sent one physical radius to all four components of the `vec4`; it does
    /// not any more, and a transcription that collapsed the pick would be
    /// describing a program nobody runs.
    ///
    /// The pick is on the **unfolded** coordinate and happens before the
    /// clamp, which is the order `FIELD`'s `picked` lines are in and the only
    /// order that can tell four corners apart: `abs()` on the next line makes
    /// every corner look like the top-left. `point.0 < half.0` is the shader's
    /// `v_coords.x < 0.5` with both sides multiplied by `tex_size.x` --
    /// `point` is `v_coords * tex_size` by this function's own contract.
    fn away(point: (f64, f64), texture: Size<i32, Physical>, radii: Corners) -> f64 {
        let half = (f64::from(texture.w) * 0.5, f64::from(texture.h) * 0.5);
        let picked = match (point.0 < half.0, point.1 < half.1) {
            (true, true) => radii.top_left,
            (false, true) => radii.top_right,
            (true, false) => radii.bottom_left,
            (false, false) => radii.bottom_right,
        };
        let r = picked.min(half.0.min(half.1));
        let p = (
            (point.0 - half.0).abs() - (half.0 - r),
            (point.1 - half.1).abs() - (half.1 - r),
        );
        p.0.max(0.0).hypot(p.1.max(0.0)) - r
    }

    /// **Every corner of what this file calls opaque is a fragment the shader
    /// leaves untouched**, checked against the shader's own arithmetic.
    ///
    /// The corners and not the middle, and that is the whole test: `away` is
    /// `-r` at *every* interior point of a quadrant -- the field saturates --
    /// so a middle sample is satisfied by an inset of zero, by an inset of one,
    /// by any inset at all. The corners of the claimed rect are the only points
    /// whose answer depends on how far it was inset.
    ///
    /// `smoothstep(-0.5, 0.5, away)` is the mask, so "untouched" is
    /// `away <= -0.5` and not `away <= 0.0`: a fragment in the softened band is
    /// partly cut, and partly cut is not opaque.
    ///
    /// **Four different radii, so each corner of the claimed rect is checked
    /// against the radius of the quadrant it actually lands in.** With
    /// `Corners::all` this test cannot see a transposition anywhere between
    /// `max_of_side` and the shader's `picked`: all four quadrants answer the
    /// same, so the rect could be inset by the wrong side's corner and still
    /// land outside a circle of the right size. The values are picked so that
    /// no two quadrants agree and so that each side's inset comes from a
    /// different corner than the one sharing its name.
    ///
    /// The last two assertions are what stop the test being vacuous. A
    /// transcription that returned some large negative number everywhere would
    /// satisfy all the rest of it; the window's own corners have to come out
    /// *outside* the shape, which is the thing that made the inset necessary --
    /// and two of them, in two quadrants with two different radii, because one
    /// is satisfied by a field that reads a single component of `corner_radius`
    /// and ignores the pick.
    #[test]
    fn the_shader_leaves_everything_this_file_claims_alone() {
        for line in FIELD {
            assert!(
                ROUNDED_CORNERS.lines().any(|source| source.trim() == line),
                "`{line}` is no longer in the shader, so `away` below is a \
                 transcription of a program that is not the one being run"
            );
        }

        let texture = Size::<i32, Physical>::from((300, 200));
        let radii = Corners {
            top_left: 6.0,
            top_right: 20.0,
            bottom_left: 12.0,
            bottom_right: 30.0,
        };

        for magnification in [1.0_f64, 0.5, 2.0, 3.7] {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a test's own window sizes, chosen to be whole"
            )]
            let drawn = Size::<i32, Physical>::from((
                (f64::from(texture.w) * magnification) as i32,
                (f64::from(texture.h) * magnification) as i32,
            ));
            let dst = Rectangle::<i32, Physical>::new((0, 0).into(), drawn);
            let claimed = opaque_of(dst, texture, radii, 1.0, true)
                .unwrap_or_else(|| panic!("{magnification}x claims nothing at all"));

            // Back into the texture's pixels, which is where the shader
            // measures: the element's rect is the texture stretched onto it.
            let back = |x: i32, y: i32| {
                (
                    f64::from(x) / f64::from(drawn.w) * f64::from(texture.w),
                    f64::from(y) / f64::from(drawn.h) * f64::from(texture.h),
                )
            };
            for (x, y) in [
                (claimed.loc.x, claimed.loc.y),
                (claimed.loc.x + claimed.size.w, claimed.loc.y),
                (claimed.loc.x, claimed.loc.y + claimed.size.h),
                (
                    claimed.loc.x + claimed.size.w,
                    claimed.loc.y + claimed.size.h,
                ),
            ] {
                let d = away(back(x, y), texture, radii);
                assert!(
                    d <= -0.5,
                    "at {magnification}x the corner ({x}, {y}) of the claimed \
                     region is {d} from the shape's edge, so the shader cuts \
                     into it and the wallpaper behind it would go undrawn"
                );
            }
        }

        assert!(
            away((0.0, 0.0), texture, radii) > 0.5,
            "the window's own top-left corner is outside the rounded shape -- \
             without this the field above could be a constant and every \
             assertion in this test would hold"
        );
        assert!(
            away((300.0, 200.0), texture, radii) > 0.5,
            "and its bottom-right, which is cut by a different radius: one \
             corner alone is satisfied by a field that ignores the quadrant \
             pick and always reads `corner_radius.x`"
        );
    }

    /// Whether the client covered its capture, which is the question
    /// `a_capture_the_client_left_translucent_is_opaque_nowhere` turns into a
    /// claim of nothing.
    ///
    /// The overlapping pair is the case that matters: two regions whose areas
    /// sum to the whole texture, arranged so that they do not cover it. An
    /// implementation that added areas up -- which is the obvious cheap one --
    /// calls that covered, and it is the shape a client with a translucent
    /// strip actually produces.
    #[test]
    fn a_capture_is_opaque_only_when_the_client_covered_all_of_it() {
        let size = Size::<i32, Physical>::from((100, 100));
        let whole = Rectangle::<i32, Physical>::new((0, 0).into(), size);
        assert!(covers(size, [whole]));
        assert!(!covers(size, []), "a client that declared nothing opaque");
        assert!(
            covers(
                size,
                [
                    Rectangle::new((0, 0).into(), (100, 60).into()),
                    Rectangle::new((0, 40).into(), (100, 60).into()),
                ]
            ),
            "two overlapping halves that do cover it"
        );
        assert!(
            !covers(
                size,
                [
                    Rectangle::new((0, 0).into(), (100, 60).into()),
                    Rectangle::new((0, 10).into(), (100, 60).into()),
                ]
            ),
            "and two that overlap enough to add up to it without covering it"
        );
        // The behaviour and not the guard: smithay delivers this one on its own,
        // so deleting `!size.is_empty()` from `covers` leaves every test here
        // green. Asserted anyway, because it is the answer that matters and
        // because the assertion is what would notice if smithay stopped giving
        // it. `covers` says why the guard stays.
        assert!(
            !covers(Size::from((0, 0)), [whole]),
            "a capture with no pixels is not opaque, whatever is claimed of it"
        );
    }
}
