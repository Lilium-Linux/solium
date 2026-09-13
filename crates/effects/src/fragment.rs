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
    /// than unimplemented. The renderer refuses it for now and says so.
    Backdrop,
}

/// The corner radius a rounded-corner program takes, in physical pixels.
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
/// Smithay's contract for a TEXTURE program, read from `shaders/mod.rs:125`
/// rather than from the doc comment on `compile_custom_texture_shader`, which
/// is wrong about the first of these:
///
/// * the source must contain a line that is exactly `//_DEFINES_` -- with the
///   trailing underscore; the doc comment omits it;
/// * the source supplies its own `#version 100`. `texture_program` does not
///   prepend one, and the built-in `texture.frag` carries its own. (The
///   *pixel* program is the one where smithay prepends it.)
///
/// `alpha` is Smithay's; `corner_radius` and `tex_size` are ours. A texture
/// program gets no `size` uniform -- only a pixel program does.
/// The distance field is the standard rounded-box one: fold the coordinate
/// into one quadrant, and measure from the centre of that corner's circle.
/// Antialiased over one pixel with `smoothstep`, because a hard cut on a
/// curve is a staircase.
pub const ROUNDED_CORNERS: &str = r"#version 100

//_DEFINES_

precision mediump float;
uniform sampler2D tex;
uniform float alpha;
uniform float corner_radius;
uniform vec2 tex_size;
varying vec2 v_coords;

void main() {
    vec4 colour = texture2D(tex, v_coords);

    // Into pixels, then into one corner's quadrant: abs() folds all four
    // corners onto one, so the distance field is written once rather than
    // four times. `v_coords` is `(tex_matrix * position).xy`, which for a
    // whole texture runs 0..1 -- see smithay's texture.vert.
    vec2 half_size = tex_size * 0.5;
    vec2 p = abs(v_coords * tex_size - half_size) - (half_size - vec2(corner_radius));
    float away = length(max(p, 0.0)) - corner_radius;

    // Every channel, not just alpha: a wayland surface is PREMULTIPLIED, so
    // colour and alpha have to be scaled together or a faded edge comes out
    // too bright. Smithay's own texture.frag does `color * alpha` for the
    // same reason.
    gl_FragColor = colour * alpha * (1.0 - smoothstep(-0.5, 0.5, away));
}
";

/// One effect on one node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// Rounded corners, with a radius in logical pixels.
    Rounded { radius: f64 },
}

impl Effect {
    /// Rounded corners at `radius` logical pixels.
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

    /// The radius, in logical pixels.
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
    pub fn is_none_effect(&self) -> bool {
        match self {
            Self::Rounded { radius } => *radius <= 0.0 || radius.is_nan(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Whole line, not `contains`: `contains("//_DEFINES")` is true of the
        // WRONG spelling too, because it is a prefix of the right one. That
        // near-miss is the bug this test exists to catch, so matching a
        // substring here would make the test agree with the defect.
        assert!(
            ROUNDED_CORNERS
                .lines()
                .any(|line| line.trim() == "//_DEFINES_"),
            "smithay replaces a line that is exactly `//_DEFINES_` with its #defines"
        );
        assert!(
            ROUNDED_CORNERS.starts_with("#version 100"),
            "texture_program does NOT prepend a version -- the built-in \
             texture.frag carries its own, and so must this"
        );
        assert!(ROUNDED_CORNERS.contains(RADIUS_UNIFORM));
        assert!(
            ROUNDED_CORNERS.contains(SIZE_UNIFORM),
            "a texture program gets no `size` uniform from smithay; only a \
             pixel program does, so this one has to declare its own"
        );
    }
}
