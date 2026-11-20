//! A geometry effect's `.vert`: one GLSL function, `vec2 sol_mesh(vec2 uv)`,
//! the place in global logical pixels where the window's point at `uv` of
//! its unit square is drawn, run on the GPU once for every point of the
//! effect's grid and read back by the compositor, which checks the grid and
//! draws the window across it.
//!
//! The file is GLSL ES 3.00, the version the compositor runs it in (it reads
//! the points back through transform feedback, which GLES 2 lacks). This
//! module holds the strings it is wrapped in, as `glsl` holds a `.frag`'s:
//! the prelude declares what the engine sets (`sol_from`, `sol_to`, the
//! progress, the axis, each param as `p_<name>`) and the helpers `sol_uv`,
//! `sol_phase` and `sol_at`, and the epilogue's `main` writes `sol_point` for
//! the grid point the vertex is. `tests::the_prelude_declares_every_param_and_numbers_the_file_from_one`.

use crate::Axis;
use crate::glsl::{Lint, ParamKind, Sources, glsl_type, identifiers, uncommented};
use crate::spec::{Severity, nearest};

/// What the compositor reads back: the varying the epilogue writes.
pub const POINT: &str = "sol_point";

/// The fragment shader a mesh program links with: nothing is rasterised, but
/// GLSL ES 3.00 links a vertex shader only beside one.
pub const MESH_FRAGMENT: &str = "#version 300 es
precision mediump float;
out vec4 sol_unused;
void main() { sol_unused = vec4(0.0); }
";

const HEAD: &str = "#version 300 es
precision highp float;
precision highp int;
// Solium's mesh prelude, api 2. Your file is the next source string, and the
// `#line 0 1` that ends this one numbers it from line 1, so a compiler's line
// numbers are your own.
in vec2 position;
uniform vec4 sol_from;
uniform vec4 sol_to;
uniform vec4 sol_monitor;
uniform vec4 sol_part;
uniform vec2 sol_grid;
uniform float sol_progress;
uniform float sol_clamped;
uniform float sol_direction;
uniform float sol_seed;
uniform float sol_scale;
uniform int sol_axis;
vec2 sol_uv() { return sol_part.xy + (sol_part.zw - sol_part.xy) * (position / sol_grid); }
float sol_phase(vec2 uv) {
    if (sol_axis == 0) { return 1.0 - uv.y; }
    if (sol_axis == 1) { return uv.y; }
    if (sol_axis == 2) { return uv.x; }
    return 1.0 - uv.x;
}
vec2 sol_at(vec4 rect, vec2 uv) { return rect.xy + uv * rect.zw; }
";

const EPILOGUE: &str = "
#line 0 2
out vec2 sol_point;
void main() {
    sol_point = sol_mesh(sol_uv());
    gl_Position = vec4(0.0, 0.0, 0.0, 1.0);
    gl_PointSize = 1.0;
}
";

/// The names the prelude and epilogue declare, without their `sol_` prefix:
/// what [`lint`] accepts. `tests::lint_names_an_unknown_param_or_name_at_its_line`.
const VOCABULARY: [&str; 16] = [
    "mesh",
    "point",
    "from",
    "to",
    "monitor",
    "part",
    "grid",
    "progress",
    "clamped",
    "direction",
    "seed",
    "scale",
    "axis",
    "uv",
    "phase",
    "at",
];

/// `sol_axis` for an axis: the order `sol_phase` reads it in.
/// `tests::sol_phase_reads_the_axis_as_rust_does`.
#[must_use]
pub fn axis_index(axis: Axis) -> i32 {
    match axis {
        Axis::Down => 0,
        Axis::Up => 1,
        Axis::Left => 2,
        Axis::Right => 3,
    }
}

/// The three vertex shader strings of a mesh program for an effect with
/// these params, its `.vert` the middle one.
/// `tests::the_prelude_declares_every_param_and_numbers_the_file_from_one`.
#[must_use]
pub fn assemble(params: &[(String, ParamKind)], user: &str) -> Sources {
    let mut prelude = String::from(HEAD);
    for (name, kind) in params {
        prelude.push_str(&format!("uniform {} p_{name};\n", glsl_type(*kind)));
    }
    prelude.push_str("#line 0 1\n");
    Sources {
        prelude,
        user: user.to_owned(),
        epilogue: EPILOGUE,
    }
}

/// What a `.vert` gets wrong that a machine with no GPU can say, by line: a
/// `p_` name that is no param, a `sol_` name that is none of the
/// prelude's, and a uniform of either kind declared by hand.
/// `tests::lint_names_an_unknown_param_or_name_at_its_line`.
#[must_use]
pub fn lint(params: &[(String, ParamKind)], user: &str) -> Vec<Lint> {
    let names: Vec<&str> = params.iter().map(|(name, _)| name.as_str()).collect();
    let mut found = Vec::new();
    for (index, line) in uncommented(user).lines().enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let words: Vec<&str> = identifiers(line).collect();
        if words.first() == Some(&"uniform")
            && let Some(name) = words.last()
            && (name.starts_with("p_") || name.starts_with("sol_"))
        {
            found.push(Lint {
                line: number,
                severity: Severity::Error,
                message: format!("`{name}` is the engine's to declare; delete this line"),
            });
            continue;
        }
        for word in words {
            if let Some(param) = word.strip_prefix("p_") {
                if !names.contains(&param) {
                    let meant = nearest(param, &names)
                        .map(|meant| format!("; did you mean `p_{meant}`?"))
                        .unwrap_or_default();
                    found.push(Lint {
                        line: number,
                        severity: Severity::Error,
                        message: format!("`{word}` is not a param of this effect{meant}"),
                    });
                }
            } else if let Some(name) = word.strip_prefix("sol_")
                && !VOCABULARY.contains(&name)
            {
                let meant = nearest(name, &VOCABULARY)
                    .map(|meant| format!("; did you mean `sol_{meant}`?"))
                    .unwrap_or_default();
                found.push(Lint {
                    line: number,
                    severity: Severity::Error,
                    message: format!("`{word}` is not part of the mesh contract{meant}"),
                });
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{assemble, axis_index, lint};
    use crate::Axis;
    use crate::glsl::ParamKind;

    #[test]
    fn the_prelude_declares_every_param_and_numbers_the_file_from_one() {
        let sources = assemble(
            &[
                ("spread".to_owned(), ParamKind::Float),
                ("tint".to_owned(), ParamKind::Vec4),
                ("n".to_owned(), ParamKind::Int),
            ],
            "vec2 sol_mesh(vec2 uv) { return sol_at(sol_from, uv); }\n",
        );
        assert!(sources.prelude.starts_with("#version 300 es\n"));
        for declared in [
            "uniform float p_spread;",
            "uniform vec4 p_tint;",
            "uniform int p_n;",
        ] {
            assert!(sources.prelude.contains(declared), "{declared}");
        }
        assert!(
            sources.prelude.ends_with("#line 0 1\n"),
            "the user's file is line 1"
        );
        assert!(sources.epilogue.contains("sol_point = sol_mesh(sol_uv());"));
        assert_eq!(sources.strings()[1], sources.user);
    }

    /// `sol_phase` reads `sol_axis` in the order [`axis_index`] gives it,
    /// each branch Rust's own `Axis::phase`.
    #[test]
    fn sol_phase_reads_the_axis_as_rust_does() {
        let head = super::HEAD;
        for (expression, axis) in [
            ("1.0 - uv.y", Axis::Down),
            ("uv.y", Axis::Up),
            ("uv.x", Axis::Left),
        ] {
            let branch = format!(
                "if (sol_axis == {}) {{ return {expression}; }}",
                axis_index(axis)
            );
            assert!(head.contains(&branch), "{branch}");
            let (u, v) = (0.25, 0.75);
            let shader = match expression {
                "1.0 - uv.y" => 1.0 - v,
                "uv.y" => v,
                _ => u,
            };
            assert!((axis.phase(u, v) - shader).abs() < 1e-12);
        }
        assert_eq!(axis_index(Axis::Right), 3);
        assert!(
            head.contains("return 1.0 - uv.x;\n}"),
            "right is what is left"
        );
        assert!((Axis::Right.phase(0.25, 0.0) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn lint_names_an_unknown_param_or_name_at_its_line() {
        let params = [("spread".to_owned(), ParamKind::Float)];
        let found = lint(
            &params,
            "vec2 sol_mesh(vec2 uv) {\n  float s = p_sprad;\n  // p_nothing in a comment\n  return sol_at(sol_form, uv) * sol_progress;\n}\nuniform float p_spread;\n",
        );
        let said: Vec<(u32, &str)> = found
            .iter()
            .map(|lint| (lint.line, lint.message.as_str()))
            .collect();
        assert_eq!(said.len(), 3, "{said:?}");
        assert!(
            said[0].0 == 2 && said[0].1.contains("`p_spread`"),
            "{said:?}"
        );
        assert!(
            said[1].0 == 4 && said[1].1.contains("`sol_from`"),
            "{said:?}"
        );
        assert!(said[2].0 == 6 && said[2].1.contains("engine's"), "{said:?}");
    }
}
