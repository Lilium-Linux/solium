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

/// The corner radius a rounded-corner program takes, in **physical** pixels.
///
/// [`Effect::radius`] is in **logical** pixels, and this is the other side of
/// that seam. A style says `radius: 12` because that is what someone types
/// into a `Pane.qml`, and a style should not have to know a monitor's scale;
/// the shader measures in texture pixels throughout and knows nothing else.
///
/// **Whoever sets this uniform does the multiply by the output scale** --
/// `crates/solium/src/pass.rs`, not this crate and not the shader. The two
/// numbers are equal on a scale-1 output, which is why getting it wrong looks
/// perfect on the machine it was written on and shows up only on a HiDPI
/// screen.
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
/// The distance field is the standard rounded-box one: fold the coordinate
/// into one quadrant, and measure from the centre of that corner's circle.
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
uniform float corner_radius;
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
    // clamps -- Effect::rounded(1e9) is accepted and is not a none-effect.
    vec2 half_size = tex_size * 0.5;
    float r = min(corner_radius, min(half_size.x, half_size.y));
    vec2 p = abs(v_coords * tex_size - half_size) - (half_size - vec2(r));
    float away = length(max(p, 0.0)) - r;

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

/// One effect on one node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// Rounded corners, with a radius in logical pixels.
    Rounded { radius: f64 },
}

impl Effect {
    /// Rounded corners at `radius` **logical** pixels.
    ///
    /// Logical because this is the number a style writes, and a style does not
    /// know what scale the window will land on. [`RADIUS_UNIFORM`] is where it
    /// becomes physical, and says who multiplies.
    #[must_use]
    pub const fn rounded(radius: f64) -> Self {
        Self::Rounded { radius }
    }

    /// What this effect needs before it can draw.
    #[must_use]
    pub const fn inputs(&self) -> Inputs {
        match self {
            Self::Rounded { .. } => Inputs::SelfTexture,
        }
    }

    /// The radius, in **logical** pixels. [`RADIUS_UNIFORM`] is the physical
    /// one, and the conversion between them is the caller's.
    #[must_use]
    pub const fn radius(&self) -> f64 {
        match self {
            Self::Rounded { radius } => *radius,
        }
    }

    /// Whether this is an effect that should not be run at all.
    ///
    /// A radius of zero is not "rounded by nothing", it is *no effect*, and
    /// the difference is a whole offscreen pass per window per frame. Every
    /// window on a machine with no styling declares one, so this is the arm
    /// that keeps the ordinary case ordinary.
    ///
    /// NaN is named rather than left to fall out of a negation. `!(r > 0.0)`
    /// says the same thing in fewer characters, but clippy rejects a negated
    /// comparison on a partially ordered type -- and its objection is the
    /// point: NaN is incomparable, so `r <= 0.0` on its own would call it an
    /// effect and buy an offscreen pass to draw nothing.
    #[must_use]
    pub const fn is_none_effect(&self) -> bool {
        match self {
            Self::Rounded { radius } => *radius <= 0.0 || radius.is_nan(),
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

    /// An effect that reads nothing draws inline; one that reads `self` needs
    /// the node rendered to a texture first. That distinction is the whole
    /// mechanism, so it is a value and not a comment.
    #[test]
    fn rounded_corners_reads_the_node_itself() {
        let effect = Effect::rounded(12.0);
        assert_eq!(effect.inputs(), Inputs::SelfTexture);
        assert!((effect.radius() - 12.0).abs() < f64::EPSILON);
    }

    /// A radius of zero is not a rounded window with no rounding: it is no
    /// effect at all, and has to stay off the pass path entirely or every
    /// window on the machine pays for a capture to be drawn square.
    #[test]
    fn a_zero_radius_is_not_an_effect() {
        assert!(Effect::rounded(0.0).is_none_effect());
        assert!(Effect::rounded(-4.0).is_none_effect());
        assert!(!Effect::rounded(1.0).is_none_effect());
        // NaN with the rest, because it is the radius that arrives from a
        // script dividing by zero and it compares false against every bound:
        // whichever way `is_none_effect` is spelled, it has to land here.
        assert!(Effect::rounded(f64::NAN).is_none_effect());
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
            format!("uniform float {RADIUS_UNIFORM};"),
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
            ROUNDED_CORNERS.contains("vec4(colour.rgb, 1.0)"),
            "NO_ALPHA has to replace the alpha channel, not sample it"
        );
    }

    /// **A radius larger than the window degrades to a stadium; it does not
    /// eat the window.**
    ///
    /// Nothing upstream clamps: `Effect::rounded(1e9)` is accepted and is not
    /// a none-effect. Unclamped, `half_size - vec2(r)` goes negative on the
    /// short axis, every fragment of the short edge reports `away >= 0`, and a
    /// 300x200 window at radius 150 loses its top and bottom rows outright.
    #[test]
    fn a_radius_larger_than_the_window_cannot_erode_it() {
        assert!(
            has_line(&format!(
                "float r = min({RADIUS_UNIFORM}, min(half_size.x, half_size.y));"
            )),
            "the radius is used unclamped, so a large one erodes the window"
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
        // the clamp. Comments are free to say its name.
        for line in ROUNDED_CORNERS.lines() {
            let code = line.split("//").next().unwrap_or_default().trim();
            if !code.contains(RADIUS_UNIFORM) {
                continue;
            }
            assert!(
                code == format!("uniform float {RADIUS_UNIFORM};")
                    || code.starts_with("float r = min("),
                "`{RADIUS_UNIFORM}` is used at `{code}`, which is neither its \
                 declaration nor the clamp -- so that part of the distance \
                 field is computed from the unclamped radius"
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
            has_line("float away = length(max(p, 0.0)) - r;"),
            "the outset half of the distance field is not written in terms of `r`"
        );

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
