//! What an effect *reads*, which is what decides whether a frame needs a pass.
//!
//! Most effects draw over what is already there and need nothing: a wavy
//! border, a glow, spikes. They are ordinary elements and this module has
//! nothing to say about them. An effect that needs the node's own pixels --
//! rounded corners masks them, a shadow is derived from their silhouette --
//! cannot be one element in a flat list, because a flat list has nowhere to
//! say "after the things below me, before the things above me".
//!
//! So `inputs` is the declaration, and the renderer reads it. This crate holds
//! the declaration and the shader text; `crates/solium/src/pass.rs` is what
//! compiles and runs it.

/// What an effect needs before it can draw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Inputs {
    /// Draws over what is there. No pass, no capture, no cost.
    Nothing,
    /// The node's own pixels, rendered to a texture first.
    SelfTexture,
    /// What is already composited *beneath* the node.
    ///
    /// Declared here and implemented nowhere. It is what blur needs, and
    /// naming it is most of why this enum exists -- an effect system whose
    /// vocabulary cannot express blur has decided blur is impossible rather
    /// than unimplemented.
    ///
    /// Nothing constructs it yet. Whatever plans the passes must refuse it out
    /// loud rather than let it fall through to drawing inline: a blur that
    /// silently renders as no blur looks like a style that failed to load, and
    /// nobody reports that as a compositor bug.
    Backdrop,
}

/// The four corner radii a rounded-corner program takes, in **physical**
/// pixels, packed `(top_left, top_right, bottom_left, bottom_right)`.
///
/// [`Effect::radii`] is in **logical** pixels, and this is the other side of
/// that seam. A style says `radius: 12` because that is what someone types
/// into a `Pane.qml`, and a style should not have to know a monitor's scale;
/// the shader measures in texture pixels throughout and knows nothing else.
///
/// **Whoever sets this uniform does the multiply by the output scale, on all
/// four** -- `crates/solium/src/pass.rs`, not this crate and not the shader.
/// The two numbers are equal on a scale-1 output, which is why getting it
/// wrong looks perfect on the machine it was written on and shows up only on
/// a HiDPI screen.
pub const RADIUS_UNIFORM: &str = "corner_radius";

/// The texture's size in physical pixels.
///
/// Ours, not Smithay's. A *pixel* program is given a `size` uniform; a
/// *texture* program is not -- the built-in `texture.frag` declares only
/// `tex`, `alpha` and `v_coords`. So a texture shader that needs to measure in
/// pixels has to be told how big it is.
pub const SIZE_UNIFORM: &str = "tex_size";

/// Rounded corners, as a fragment program over the node's own texture.
///
/// Smithay's contract for a TEXTURE program, read from `shaders/mod.rs`
/// rather than from the doc comment on `compile_custom_texture_shader`, which
/// is wrong about the first of these:
///
/// * the source must contain a line that is exactly `//_DEFINES_` -- with the
///   trailing underscore; the doc comment omits it;
/// * the source supplies its own `#version 100`. `texture_program` does not
///   prepend one, and the built-in `texture.frag` carries its own. (The
///   *pixel* program is the one where smithay prepends it.)
/// * it is compiled **six times**, not once, and has to behave under each set
///   of `#define`s. Three define sets -- `&[]`, `&[NO_ALPHA]`, `&[EXTERNAL]`
///   -- and `texture_program` links each of them *twice*, once plain and once
///   with `DEBUG_FLAGS` chained on (`shaders/mod.rs:215-217` builds the three; the two `link_program` calls at `:142-143` are what double them). Three is the
///   number of variants `variant_for_format` picks between; six is the number
///   that have to compile, and a shader that only builds without the debug
///   define fails at renderer construction and takes the session with it. See
///   `the_shader_handles_every_variant_smithay_compiles_it_into`, and
///   `texture.frag`, which is the model this mirrors.
///
/// `alpha` is Smithay's; `corner_radius` and `tex_size` are ours. A texture
/// program gets no `size` uniform -- only a pixel program does.
/// The distance field is the standard rounded-box one: pick a radius for the
/// fragment's quadrant, fold the coordinate into that quadrant, and measure
/// from the centre of that corner's circle.
pub const ROUNDED_CORNERS: &str = r"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

// highp where the hardware has it, which is not a nicety here.
//
// GLES 2 guarantees mediump only 10 mantissa bits. This shader multiplies a
// normalised coordinate back up into pixels -- `v_coords * tex_size` -- so one
// ULP at 3840px is about 3.75px, seven times the width of the smoothstep that
// antialiases the arc. The corner would wobble and its edge come apart, on a
// 4K screen only.
//
// Mesa evaluates mediump at fp32, so none of that is visible on the machine
// this was written on. Same shape as the logical-versus-physical radius above:
// a defect whose only symptom is on hardware the author does not have.
// `GL_FRAGMENT_PRECISION_HIGH` is the compiler's own answer to whether highp
// exists in a fragment shader, so this asks rather than assumes.
//
// smithay's `texture.frag` settles for mediump and is not a precedent: it
// samples at `v_coords` and never scales a normalised coordinate back into
// pixel space, so it has no bits to lose.
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
uniform vec4 corner_radius;
uniform vec2 tex_size;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec4 colour = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    // Forced opaque rather than sampled. The X byte of an XRGB8888 buffer is
    // undefined, so a client that leaves it at zero draws a fully invisible
    // window if it is multiplied in -- and XRGB8888 is the ordinary opaque
    // case, not an exotic one. Smithay picks this variant itself, from the
    // buffer's format.
    colour = vec4(colour.rgb, 1.0) * alpha;
#else
    // Every channel, not just alpha: a wayland surface is PREMULTIPLIED, so
    // colour and alpha have to be scaled together or a faded edge comes out
    // too bright. Smithay's own texture.frag does `color * alpha` for the
    // same reason.
    colour = colour * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        colour = vec4(0.0, 0.2, 0.0, 0.2) + colour * 0.8;
#endif

    // Into pixels, then into one corner's quadrant: abs() folds all four
    // corners onto one, so the distance field is written once rather than
    // four times. `v_coords` is `(tex_matrix * position).xy`, which for a
    // whole texture runs 0..1 -- see smithay's texture.vert.
    //
    // Clamped to the shorter half, and the field is written in terms of the
    // clamp. Unclamped, a radius past min(w,h)/2 pushes each corner circle's
    // centre outside the rectangle and the field calls every fragment of the
    // short edge outside: a 300x200 window at radius 150 loses its whole top
    // and bottom rows instead of degrading to a stadium. Nothing upstream
    // clamps -- Effect::rounded(Corners::all(1e9)) is accepted and is not a
    // none-effect.
    vec2 half_size = tex_size * 0.5;

    // Which corner this fragment belongs to, decided on the UNFOLDED
    // coordinate. `abs()` below makes all four look like the top-left, which
    // is what lets one expression draw four corners -- and is exactly why the
    // radius has to be chosen first. `corner_radius` is (tl, tr, bl, br).
    float picked = (v_coords.x < 0.5)
        ? ((v_coords.y < 0.5) ? corner_radius.x : corner_radius.z)
        : ((v_coords.y < 0.5) ? corner_radius.y : corner_radius.w);
    float r = min(picked, min(half_size.x, half_size.y));
    vec2 p = abs(v_coords * tex_size - half_size) - (half_size - vec2(r));

    // The exact rounded-box distance, **interior term and all**.
    //
    // `length(max(p, 0.0)) - r` on its own is only the OUTSIDE half of the
    // field. It is right on the arcs and right past every edge, and inside the
    // shape it saturates: both components of `p` are negative there, so it
    // reports a flat `-r` at every interior fragment instead of that
    // fragment's real distance to the edge. While a window had one radius that
    // was merely imprecise -- anything at `r >= 0.5` is past the smoothstep's
    // far end and comes out fully opaque either way, which is why half a pixel
    // was the bound everything downstream was written against.
    //
    // With a radius per quadrant it stops being imprecise and starts being
    // wrong, because `r` can now be 0 on one corner while the others round. A
    // zero radius makes the interior report exactly 0 -- the smoothstep's
    // MIDPOINT -- so that quadrant, a quarter of the window, is drawn at 50%
    // alpha rather than squared off. Measured on a real GPU through
    // `dev/wirecheck`: `corner_radius = (0, 16, 16, 16)` came back at alpha
    // 127 across every pixel of the top-left quadrant, and at 228 for a radius
    // of 0.3. A square-topped window is the headline case for per-corner
    // radii, not an exotic one -- `panes/flush/` is exactly that shape.
    //
    // `min(max(p.x, p.y), 0.0)` is that missing term, and adding it changes
    // nothing anywhere else: it is exactly 0.0 wherever either component of
    // `p` is positive, which is every fragment on an arc, along a straight
    // edge, or outside the shape. Checked and not merely argued -- all four
    // quadrants of a 64x64 at r=16 hash byte-identical across the change.
    //
    // It also settles where the shape's own boundary lands. A fragment centre
    // sits half a pixel inside the edge, so the outermost row now reports
    // exactly -0.5 for any `r` -- the smoothstep's near end, fully opaque --
    // which is what lets `pass::opaque_inside` claim an uncut side with an
    // inset of zero.
    float away = min(max(p.x, p.y), 0.0) + length(max(p, 0.0)) - r;

    // The mask goes last, after the tint. The tint ADDS a constant, so a
    // fragment masked to zero before it would be painted back in and the cut
    // corners would reappear in green whenever debug flags are on.
    //
    // The one-unit smoothstep is not a one-*pixel* antialias: `away` is in
    // texture pixels, so the softened band is one screen pixel wide only when
    // the texture is drawn at 1:1. Magnified it is a visible blur, minified it
    // aliases. The scale-correct form is smoothstep(-w, w, away) with
    // w = fwidth(away), which under #version 100 needs
    // GL_OES_standard_derivatives; until that extension is requested, this
    // buys a soft edge at native scale and a wrong-width one everywhere else.
    gl_FragColor = colour * (1.0 - smoothstep(-0.5, 0.5, away));
}
";

/// Four corner radii, independently -- what lets a window be square-topped
/// and round-bottomed, or any other combination, rather than one radius
/// applied uniformly.
///
/// In **logical** pixels throughout, the same seam as [`Effect::radii`]: a
/// style writes these, and does not know what scale the window will land on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corners {
    pub top_left: f64,
    pub top_right: f64,
    pub bottom_left: f64,
    pub bottom_right: f64,
}

impl Corners {
    /// One radius, all four corners. The common case, and the constructor
    /// that used to be [`Effect::rounded`]'s whole job before a window was
    /// allowed to differ corner to corner.
    #[must_use]
    pub const fn all(radius: f64) -> Self {
        Self {
            top_left: radius,
            top_right: radius,
            bottom_left: radius,
            bottom_right: radius,
        }
    }

    /// Whether every corner is `<= 0.0` or NaN.
    ///
    /// NaN is named rather than left to fall out of a negation, the same
    /// spelling the single-radius [`Effect::is_none_effect`] used before this
    /// existed: `!(r > 0.0)` says the same thing in fewer characters, but
    /// clippy rejects a negated comparison on a partially ordered type -- and
    /// its objection is the point: NaN is incomparable, so `r <= 0.0` on its
    /// own would call it rounded and buy an offscreen pass to draw nothing.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        (self.top_left <= 0.0 || self.top_left.is_nan())
            && (self.top_right <= 0.0 || self.top_right.is_nan())
            && (self.bottom_left <= 0.0 || self.bottom_left.is_nan())
            && (self.bottom_right <= 0.0 || self.bottom_right.is_nan())
    }

    /// Each side, measured by the larger of the two corners that touch it --
    /// `(top, right, bottom, left)`. A square-topped, round-bottomed window
    /// must not give up its top rows to a corner it does not have.
    #[must_use]
    pub fn max_of_side(&self) -> (f64, f64, f64, f64) {
        let top = self.top_left.max(self.top_right);
        let right = self.top_right.max(self.bottom_right);
        let bottom = self.bottom_left.max(self.bottom_right);
        let left = self.top_left.max(self.bottom_left);
        (top, right, bottom, left)
    }

    /// The one number that stands for all four, for the `clientRadius` a layer
    /// is still told. The *largest*, because that field's only in-tree use is
    /// an outward hug (`PaneStyle.qml`: `clientRadius + 2`, "hug it from
    /// outside"), and a hug has to clear the biggest cut or it clips into it.
    #[must_use]
    pub fn largest(&self) -> f64 {
        self.top_left
            .max(self.top_right)
            .max(self.bottom_left)
            .max(self.bottom_right)
    }
}

/// One effect on one node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// Rounded corners, with a radius per corner, in logical pixels.
    Rounded { radii: Corners },
}

impl Effect {
    /// Rounded corners at `radii` **logical** pixels, one value per corner.
    ///
    /// Logical because this is the number a style writes, and a style does not
    /// know what scale the window will land on. [`RADIUS_UNIFORM`] is where it
    /// becomes physical, and says who multiplies.
    ///
    /// A single shared radius is the common case, and that convenience now
    /// lives on [`Corners::all`] rather than here: `Effect::rounded(Corners::all(12.0))`
    /// is the old one-number behaviour.
    #[must_use]
    pub const fn rounded(radii: Corners) -> Self {
        Self::Rounded { radii }
    }

    /// What this effect needs before it can draw.
    #[must_use]
    pub const fn inputs(&self) -> Inputs {
        match self {
            Self::Rounded { .. } => Inputs::SelfTexture,
        }
    }

    /// The radii, in **logical** pixels, one per corner. [`RADIUS_UNIFORM`] is
    /// the physical one, and the conversion between them is the caller's.
    #[must_use]
    pub const fn radii(&self) -> Corners {
        match self {
            Self::Rounded { radii } => *radii,
        }
    }

    /// The one number [`Corners::largest`] gives this effect's radii -- see
    /// there for why the largest. A one-line delegate rather than a second
    /// copy of the max chain, so the reasoning has exactly one home.
    #[must_use]
    pub fn largest(&self) -> f64 {
        self.radii().largest()
    }

    /// Whether this is an effect that should not be run at all.
    ///
    /// No corner rounded is not "rounded by nothing", it is *no effect*, and
    /// the difference is a whole offscreen pass per window per frame. Every
    /// window on a machine with no styling declares one, so this is the arm
    /// that keeps the ordinary case ordinary. See [`Corners::is_none`] for the
    /// per-corner rule this now delegates to, NaN included.
    #[must_use]
    pub const fn is_none_effect(&self) -> bool {
        match self {
            Self::Rounded { radii } => radii.is_none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether the shader has this exact line, ignoring indentation.
    ///
    /// A whole line rather than `contains`, everywhere, because every name in
    /// this shader is a substring of something else that is legitimately
    /// present. `contains(RADIUS_UNIFORM)` passes for `RADIUS_UNIFORM =
    /// "radius"`, since `corner_radius` contains it; `contains(SIZE_UNIFORM)`
    /// passes for `"size"`, which is the *pixel* program's uniform name that
    /// this module exists to work around; and `contains("//_DEFINES")` passes
    /// for the wrong marker, which is a prefix of the right one. All three
    /// compile. All three fail only on a GPU, silently.
    fn has_line(wanted: &str) -> bool {
        ROUNDED_CORNERS.lines().any(|line| line.trim() == wanted)
    }

    /// Four corners, and the shader has to tell them apart. A single radius is
    /// the common case and gets a constructor; it is not the only case.
    #[test]
    fn corners_can_differ() {
        let finder = Corners {
            top_left: 0.0,
            top_right: 0.0,
            bottom_left: 12.0,
            bottom_right: 12.0,
        };
        let effect = Effect::rounded(finder);
        assert_eq!(effect.radii(), finder);
        assert!(
            !effect.is_none_effect(),
            "two corners rounded is still an effect"
        );
    }

    /// All four zero is *no effect*, and that is what keeps the ordinary
    /// window off the pass path entirely.
    #[test]
    fn every_corner_zero_is_no_effect() {
        assert!(Effect::rounded(Corners::all(0.0)).is_none_effect());
        assert!(Effect::rounded(Corners::all(-3.0)).is_none_effect());
        assert!(Effect::rounded(Corners::all(f64::NAN)).is_none_effect());
        // But ONE corner is enough to be one.
        let one = Corners {
            top_left: 8.0,
            ..Corners::all(0.0)
        };
        assert!(!Effect::rounded(one).is_none_effect());
    }

    /// Each side is inset by the larger of the two corners touching it. The
    /// asymmetry is the point: a square-topped, round-bottomed window must not
    /// give up its top rows.
    #[test]
    fn a_side_is_measured_by_its_larger_corner() {
        let corners = Corners {
            top_left: 0.0,
            top_right: 0.0,
            bottom_left: 12.0,
            bottom_right: 20.0,
        };
        let (top, right, bottom, left) = corners.max_of_side();
        assert!(
            (top - 0.0).abs() < f64::EPSILON,
            "no top corner is cut, so no top rows are lost"
        );
        assert!(
            (right - 20.0).abs() < f64::EPSILON,
            "the right side touches top-right and bottom-right"
        );
        assert!((bottom - 20.0).abs() < f64::EPSILON);
        assert!((left - 12.0).abs() < f64::EPSILON);
    }

    /// The shader picks a radius per quadrant BEFORE `abs()` folds the
    /// coordinate, which is the one line that makes four radii possible at
    /// all. Pinned by text here and drawn for real in wirecheck.
    #[test]
    fn the_shader_selects_a_radius_per_quadrant() {
        assert!(
            has_line("uniform vec4 corner_radius;"),
            "four radii, as tl/tr/bl/br"
        );
        assert!(
            has_line("float picked = (v_coords.x < 0.5)"),
            "the quadrant is chosen from the unfolded coordinate; after `abs()` \
             every corner looks like the top-left and the four are indistinguishable"
        );
    }

    /// `largest` is what a layer's own `clientRadius` still gets -- see
    /// `Corners::largest`. It has to be the biggest of the four, not the
    /// smallest or an average, because `PaneStyle.qml`'s `clientRadius + 2`
    /// hug has to clear the biggest cut or it clips into it.
    ///
    /// On a bare `Corners` directly, because that is the shape a later task
    /// holds with no `Effect` around it; `Effect::largest` is checked too,
    /// since it is a caller of this and not a second implementation.
    #[test]
    fn largest_is_the_biggest_of_the_four() {
        let corners = Corners {
            top_left: 4.0,
            top_right: 20.0,
            bottom_left: 12.0,
            bottom_right: 0.0,
        };
        assert!((corners.largest() - 20.0).abs() < f64::EPSILON);
        assert!((Effect::rounded(corners).largest() - 20.0).abs() < f64::EPSILON);
    }

    /// An effect that reads nothing draws inline; one that reads `self` needs
    /// the node rendered to a texture first. That distinction is the whole
    /// mechanism, so it is a value and not a comment.
    #[test]
    fn rounded_corners_reads_the_node_itself() {
        let effect = Effect::rounded(Corners::all(12.0));
        assert_eq!(effect.inputs(), Inputs::SelfTexture);
        assert_eq!(effect.radii(), Corners::all(12.0));
    }

    /// The shader is handed to Smithay, whose contract for a *texture* program
    /// is not the one its own documentation states. Read from the source, not
    /// the doc comment:
    ///
    /// | | texture program | pixel program |
    /// |---|---|---|
    /// | marker | `//_DEFINES_` | `//_DEFINES_` |
    /// | `#version` | **the shader supplies it** | smithay prepends it |
    /// | `size` uniform | **not provided** | provided |
    ///
    /// `gles/mod.rs:1964` says the marker is `//_DEFINES`, without the
    /// trailing underscore. `shaders/mod.rs:125` is what actually runs and it
    /// replaces `//_DEFINES_`. A shader carrying the documented spelling has
    /// its marker left in place as a comment, compiles, and then fails to link
    /// because the `#define`s it needed were never substituted.
    ///
    /// None of this fails until there is a GPU, which is why it is asserted
    /// here and compiled for real in wirecheck.
    #[test]
    fn the_shader_is_shaped_the_way_smithay_actually_requires() {
        assert!(
            has_line("//_DEFINES_"),
            "smithay replaces a line that is exactly `//_DEFINES_` with its #defines"
        );
        assert!(
            ROUNDED_CORNERS.starts_with("#version 100"),
            "texture_program does NOT prepend a version -- the built-in \
             texture.frag carries its own, and so must this"
        );

        // The declaration line, built from the constant, so the name the
        // compositor will pass to `UniformName::new` and the name the shader
        // declares cannot drift apart. See `has_line` for why not `contains`.
        for declaration in [
            format!("uniform vec4 {RADIUS_UNIFORM};"),
            format!("uniform vec2 {SIZE_UNIFORM};"),
        ] {
            assert!(
                has_line(&declaration),
                "the shader has no `{declaration}`, so GetUniformLocation \
                 returns -1 and the uniform is silently never set"
            );
        }
    }

    /// **Smithay compiles this shader three times, not once.**
    ///
    /// `shaders/mod.rs` builds a variant for each of `&[]`, `&[NO_ALPHA]` and
    /// `&[EXTERNAL]` eagerly, and `variant_for_format` picks between them per
    /// texture: variant 1 for any known format with `has_alpha == false`,
    /// variant 2 whenever the format is `None`, which is how an external
    /// texture arrives. Neither is exotic -- XRGB8888 is the ordinary opaque
    /// window, and a hardware-decoded video surface binds
    /// `TEXTURE_EXTERNAL_OES`.
    ///
    /// A shader that ignores the defines compiles perfectly and then
    /// misbehaves on a GPU: an XRGB window drawn with its undefined X byte as
    /// alpha is *invisible*, and an external texture read through a
    /// `sampler2D` is *black*. `texture.frag` is the model this mirrors.
    /// The precision guard, whose absence no test on this machine can feel.
    ///
    /// Mesa evaluates `mediump` at fp32, so removing this changes nothing here
    /// and everything on a 4K screen with a conformant driver -- one mediump
    /// ULP at 3840px is ~3.75px, against a half-pixel antialiasing band.
    #[test]
    fn the_shader_asks_for_highp_where_it_exists() {
        assert!(
            has_line("#ifdef GL_FRAGMENT_PRECISION_HIGH"),
            "the shader scales a normalised coordinate into pixels, so it has \
             to ask for highp rather than take mediump's ten mantissa bits"
        );
        assert!(has_line("precision highp float;"));
        assert!(
            has_line("precision mediump float;"),
            "and still falls back: highp in a fragment shader is optional in GLES 2"
        );
    }

    #[test]
    fn the_shader_handles_every_variant_smithay_compiles_it_into() {
        for required in [
            "#if defined(NO_ALPHA)",
            "#if defined(EXTERNAL)",
            "#extension GL_OES_EGL_image_external : require",
            "uniform samplerExternalOES tex;",
            "uniform sampler2D tex;",
        ] {
            assert!(
                has_line(required),
                "no `{required}`: one of smithay's three variants would draw wrong"
            );
        }
        // And the NO_ALPHA arm has to *do* something. A branch that takes the
        // same path under a different name satisfies every assertion above and
        // still draws an invisible window.
        assert!(
            has_line("colour = vec4(colour.rgb, 1.0) * alpha;"),
            "NO_ALPHA has to replace the alpha channel, not sample it"
        );
    }

    /// **A radius larger than the window degrades to a stadium; it does not
    /// eat the window.**
    ///
    /// Nothing upstream clamps: `Effect::rounded(Corners::all(1e9))` is
    /// accepted and is not a none-effect. Unclamped, `half_size - vec2(r)`
    /// goes negative on the short axis, every fragment of the short edge
    /// reports `away >= 0`, and a 300x200 window at radius 150 loses its top
    /// and bottom rows outright.
    #[test]
    fn a_radius_larger_than_the_window_cannot_erode_it() {
        assert!(
            has_line("float r = min(picked, min(half_size.x, half_size.y));"),
            "the picked per-corner radius is used unclamped, so a large one erodes the window"
        );
        // And nothing walks around the clamp. Stated as two properties rather
        // than as a count of the uniform's name, which was the first attempt
        // and was wrong in both directions: a comment inside the shader string
        // that happened to mention `corner_radius` pushed the count to three
        // and failed with "part of the distance field is unclamped", which is
        // a wrong diagnosis of a harmless edit -- and a bypass that replaced
        // half the field with a literal kept the count at two and passed.
        //
        // First: the raw uniform appears on no line but its declaration and
        // the two branches of the per-quadrant pick that feeds the clamp --
        // the clamp itself now reads `picked`, not `corner_radius`, which is
        // why the assertion above pins the clamp's own line directly instead
        // of walking to it here. Comments are free to say the uniform's name.
        for line in ROUNDED_CORNERS.lines() {
            let code = line.split("//").next().unwrap_or_default().trim();
            if !code.contains(RADIUS_UNIFORM) {
                continue;
            }
            assert!(
                code == format!("uniform vec4 {RADIUS_UNIFORM};")
                    || code == "? ((v_coords.y < 0.5) ? corner_radius.x : corner_radius.z)"
                    || code == ": ((v_coords.y < 0.5) ? corner_radius.y : corner_radius.w);",
                "`{RADIUS_UNIFORM}` is used at `{code}`, which is neither its \
                 declaration nor a branch of the per-quadrant pick -- so that \
                 part of the distance field reads the radius somewhere the \
                 clamp cannot reach"
            );
        }
        // Second: both halves of the field are written in terms of the clamped
        // copy. The check above cannot see this -- a half replaced by a
        // literal mentions the uniform nowhere and would pass it.
        assert!(
            has_line("vec2 p = abs(v_coords * tex_size - half_size) - (half_size - vec2(r));"),
            "the inset half of the distance field is not written in terms of `r`"
        );
        assert!(
            has_line("float away = min(max(p.x, p.y), 0.0) + length(max(p, 0.0)) - r;"),
            "this is not the exact rounded-box distance written in terms of `r`. \
             It needs BOTH halves: the interior term `min(max(p.x, p.y), 0.0)`, \
             which is what a fragment inside the shape is measured by, and the \
             outset half `length(max(p, 0.0)) - r`, which is the arcs and \
             everything past an edge"
        );
        // And the interior term is part of that line, pinned by the whole-line
        // match above rather than by a second assertion -- but it is worth
        // naming here, because dropping it is the one edit that leaves every
        // *other* assertion in this module green. `length(max(p, 0.0)) - r`
        // alone reports a flat `-r` inside the shape, which at `r == 0` is the
        // smoothstep's midpoint: a square corner's whole quadrant at 50%
        // alpha, measured at 127 on a real GPU. See the comment on the line
        // itself.

        // The two lines that decide where the corner circle sits and how wide
        // the antialiased band is. Neither was pinned here until Task 5's
        // re-review found it: a transcription of this field in
        // `solium::pass` was the ONLY thing in the workspace that noticed
        // `tex_size * 0.5` becoming `* 0.4`, and it lives in another crate.
        // A shader's own crate should be the thing that catches an edit to it.
        assert!(
            has_line("vec2 half_size = tex_size * 0.5;"),
            "the corner circles are placed from the half-size; at anything \
             but 0.5 they are not in the corners"
        );
        assert!(
            has_line("gl_FragColor = colour * (1.0 - smoothstep(-0.5, 0.5, away));"),
            "the antialias band is one texel wide and centred on the edge; \
             widening it makes the whole window translucent at small radii"
        );
    }
}
