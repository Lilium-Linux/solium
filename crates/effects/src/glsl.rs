//! The GLSL every effect `.frag` is compiled with (\[16\] §2's contract).
//!
//! A program is three source strings: [`prelude`], the user's file untouched,
//! and an epilogue that calls `sol_effect`. Mesa joins the strings into one
//! buffer before compiling (`shader_source` in `shaderapi.c`), so the prelude
//! ends with `#line 0 1` and the epilogue begins with `#line 0 2`: under GLSL
//! ES 1.00 §3.4 compiling continues at line `line + 1` of the string named, so
//! a driver numbers the user's file from line 1 of string 1, and the string
//! index says whose the error was
//! (`tests::the_users_file_is_its_own_source_string`,
//! `tests::nvidia_mesa_and_angle_logs_map_to_the_users_line`). A driver that
//! applies GLSL ES 3.00's rule instead, where the next line is `line`, numbers
//! it from 0 (NVIDIA, wirecheck case 12a): [`line_shift`] reads which rule a
//! driver follows from its log of [`line_probe`]
//! (`tests::a_drivers_line_rule_is_read_from_the_probe`).

use crate::spec::{Severity, Value};

/// The contract version a file names as `api = 1`.
pub const API: u32 = 1;

/// Which program a `.frag` is compiled into: a pass into a pooled target, or
/// the warp, sampling a capture through a mesh (X1.6's pixels).
/// `tests::the_users_file_is_its_own_source_string`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Pass,
    Warp,
}

/// The GLSL type of a param's uniform.
/// `tests::the_prelude_declares_every_param_as_its_kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamKind {
    Float,
    Int,
    Vec4,
}

/// A param's uniform kind, from its default; a word has none.
/// `tests::a_params_kind_is_its_defaults`.
pub fn kind_of(value: &Value) -> Option<ParamKind> {
    match value {
        Value::Number(_) => Some(ParamKind::Float),
        Value::Int(_) | Value::Bool(_) => Some(ParamKind::Int),
        Value::Vec4(_) => Some(ParamKind::Vec4),
        Value::Word(_) => None,
    }
}

/// What one pass's program is compiled against.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Signature {
    pub host: Host,
    /// The effect's params, plus a `repeat`'s `as` name for the passes in it.
    pub params: Vec<(String, ParamKind)>,
    /// The named textures this pass reads, bound to units 1 onwards in order.
    pub uses: Vec<String>,
    /// Every texture name the effect could read: inputs, saves, states. A
    /// known name not in `uses` reads transparent
    /// (`tests::a_name_missing_from_uses_reads_transparent_and_has_0`).
    pub known: Vec<String>,
}

/// One program's three fragment source strings.
/// `tests::the_users_file_is_its_own_source_string`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    pub prelude: String,
    pub user: String,
    pub epilogue: &'static str,
}

impl Sources {
    /// The strings in the order the driver numbers them: 0, 1 and 2.
    /// `tests::a_programs_key_is_its_three_strings_and_its_vertex_shader`.
    pub fn strings(&self) -> [&str; 3] {
        [&self.prelude, &self.user, self.epilogue]
    }

    /// The program cache's key: these three strings and the vertex shader.
    /// `tests::a_programs_key_is_its_three_strings_and_its_vertex_shader`.
    pub fn key(&self, vertex: &str) -> u64 {
        content_hash(&[
            vertex.as_bytes(),
            self.prelude.as_bytes(),
            self.user.as_bytes(),
            self.epilogue.as_bytes(),
        ])
    }
}

/// A pass's vertex shader: a full-target quad, `uv` (0,0) at the top-left of
/// the target's first row (wirecheck case 12d).
pub const PASS_VERTEX: &str = "#version 100
attribute vec2 position;
varying highp vec2 v_uv;
void main() {
    v_uv = position;
    gl_Position = vec4(position * 2.0 - 1.0, 0.0, 1.0);
}
";

const PASS_EPILOGUE: &str = "
#line 0 2
varying highp vec2 v_uv;
void main() { gl_FragColor = sol_effect(v_uv); }
";

/// The warp's: the same divide its own fragment shader does
/// (`warp/gl.rs`), so one `.frag` runs in either host.
/// `tests::the_users_file_is_its_own_source_string`.
const WARP_EPILOGUE: &str = "
#line 0 2
uniform float alpha;
varying highp vec3 v_uvq;
void main() { gl_FragColor = sol_effect(v_uvq.xy / v_uvq.z) * alpha; }
";

const HEAD: &str = "#version 100
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
// Solium's effect prelude, api 1. Your file is the next source string, and
// the `#line 0 1` that ends this one numbers it from line 1 even where a
// driver joins the strings, so a compiler's line numbers are your own.
uniform sampler2D sol_tex_sampler;
uniform vec4 sol_tex_box;
uniform vec4 sol_tex_clamp;
uniform vec2 sol_texel;
uniform vec2 sol_size;
uniform vec4 sol_content;
uniform vec2 sol_box_px;
uniform vec4 sol_radii;
uniform float sol_progress;
uniform float sol_clamped;
uniform float sol_direction;
uniform float sol_seed;
uniform float sol_time;
vec4 sol_tex(vec2 uv) {
    return texture2D(sol_tex_sampler, clamp(sol_tex_box.xy + uv * sol_tex_box.zw, sol_tex_clamp.xy, sol_tex_clamp.zw));
}
vec2 sol_to_content(vec2 uv) { return (uv - sol_content.xy) * sol_box_px; }
vec2 sol_to_uv(vec2 px) { return sol_content.xy + px / sol_box_px; }
float sol_sdf_rrect(vec2 px, vec2 size, vec4 radii) {
    vec2 half_size = size * 0.5;
    vec2 at = px - half_size;
    float picked = at.x < 0.0 ? (at.y < 0.0 ? radii.x : radii.z) : (at.y < 0.0 ? radii.y : radii.w);
    float r = min(picked, min(half_size.x, half_size.y));
    vec2 p = abs(at) - (half_size - vec2(r));
    return min(max(p.x, p.y), 0.0) + length(max(p, 0.0)) - r;
}
float sol_sdf(vec2 px) { return sol_sdf_rrect(px, sol_content.zw * sol_box_px, sol_radii); }
float sol_shape(vec2 uv) { return clamp(0.5 - sol_sdf(sol_to_content(uv)), 0.0, 1.0); }
float sol_hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float sol_noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(sol_hash(i), sol_hash(i + vec2(1.0, 0.0)), u.x),
               mix(sol_hash(i + vec2(0.0, 1.0)), sol_hash(i + vec2(1.0, 1.0)), u.x), u.y);
}
";

/// The names the head declares, without their `sol_` prefix: what [`lint`]
/// accepts besides the known texture names (`tests::lint_names_an_undeclared_param_at_its_line`
/// reads `sol_tex` and `sol_effect` with no lint), and what a saved name may
/// not be (`stage::tests::a_saved_name_is_a_new_glsl_name`).
pub(crate) const VOCABULARY: [&str; 22] = [
    "effect",
    "tex",
    "tex_sampler",
    "tex_box",
    "tex_clamp",
    "texel",
    "size",
    "content",
    "box_px",
    "radii",
    "progress",
    "clamped",
    "direction",
    "seed",
    "time",
    "to_content",
    "to_uv",
    "sdf_rrect",
    "sdf",
    "shape",
    "hash",
    "noise",
];

/// A known texture's sampler, declared only while a pass uses it.
/// `tests::a_name_missing_from_uses_reads_transparent_and_has_0`.
pub fn sampler(name: &str) -> String {
    format!("sol_{name}_sampler")
}

/// A known texture's box: where its picture lies in its texture.
/// `tests::lint_reads_a_names_sampler_and_box_as_that_name`.
pub fn box_of(name: &str) -> String {
    format!("sol_{name}_box")
}

fn glsl_type(kind: ParamKind) -> &'static str {
    match kind {
        ParamKind::Float => "float",
        ParamKind::Int => "int",
        ParamKind::Vec4 => "vec4",
    }
}

/// The prelude for one pass. `tests::the_prelude_declares_every_param_as_its_kind`,
/// `tests::a_name_missing_from_uses_reads_transparent_and_has_0`,
/// `tests::the_prelude_sdf_keeps_the_interior_term_and_the_clamp`.
pub fn prelude(signature: &Signature) -> String {
    let mut text = String::from(HEAD);
    for name in &signature.known {
        if signature.uses.contains(name) {
            text.push_str(&format!(
                "uniform sampler2D {sampler};\nuniform vec4 {boxed};\nvec4 sol_{name}(vec2 uv) {{ return texture2D({sampler}, {boxed}.xy + uv * {boxed}.zw); }}\n#define SOL_HAS_{name} 1\n",
                sampler = sampler(name),
                boxed = box_of(name),
            ));
        } else {
            text.push_str(&format!(
                "vec4 sol_{name}(vec2 uv) {{ return vec4(0.0); }}\n#define SOL_HAS_{name} 0\n"
            ));
        }
    }
    for (name, kind) in &signature.params {
        text.push_str(&format!("uniform {} p_{name};\n", glsl_type(*kind)));
    }
    // Last, always: the user's file is line 1 of string 1 (Ruling 6).
    // `tests::the_users_file_is_its_own_source_string`.
    text.push_str("#line 0 1\n");
    text
}

/// The three strings of one program. `tests::the_users_file_is_its_own_source_string`.
pub fn assemble(signature: &Signature, user: &str) -> Sources {
    Sources {
        prelude: prelude(signature),
        user: user.to_owned(),
        epilogue: match signature.host {
            Host::Pass => PASS_EPILOGUE,
            Host::Warp => WARP_EPILOGUE,
        },
    }
}

/// One problem a `.frag` has that needs no GPU to see.
/// `tests::lint_names_an_undeclared_param_at_its_line`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lint {
    pub line: u32,
    pub severity: Severity,
    pub message: String,
}

/// `text` with every comment blanked, newlines kept, so line numbers hold.
/// `tests::lint_skips_comments`.
fn uncommented(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('/', Some('/')) => {
                for rest in chars.by_ref() {
                    if rest == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for rest in chars.by_ref() {
                    if rest == '\n' {
                        out.push('\n');
                    }
                    if last == '*' && rest == '/' {
                        break;
                    }
                    last = rest;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Every identifier on a line.
fn identifiers(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
}

/// What a `.frag` gets wrong that a machine with no GPU can say, by line.
/// `tests::lint_names_an_undeclared_param_at_its_line`,
/// `tests::lint_refuses_a_uniform_the_engine_declares`,
/// `tests::lint_warns_about_a_known_name_missing_from_uses`,
/// `tests::lint_reads_a_names_sampler_and_box_as_that_name`.
pub fn lint(signature: &Signature, user: &str) -> Vec<Lint> {
    let params: Vec<&str> = signature
        .params
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let is_known = |name: &str| signature.known.iter().any(|each| each == name);
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
                if !params.contains(&param) {
                    let meant = crate::spec::nearest(param, &params)
                        .map(|meant| format!("; did you mean `p_{meant}`?"))
                        .unwrap_or_default();
                    found.push(Lint {
                        line: number,
                        severity: Severity::Error,
                        message: format!("`{word}` is not a param of this effect{meant}"),
                    });
                }
            } else if let Some(name) = word
                .strip_prefix("sol_")
                .or_else(|| word.strip_prefix("SOL_HAS_"))
            {
                // `sol_<name>_sampler` and `sol_<name>_box` are `<name>`'s.
                // `tests::lint_reads_a_names_sampler_and_box_as_that_name`.
                let base = ["_sampler", "_box"]
                    .iter()
                    .find_map(|suffix| name.strip_suffix(suffix).filter(|base| is_known(base)))
                    .unwrap_or(name);
                let used = signature.uses.iter().any(|each| each == base);
                if is_known(base) {
                    if !used && word.starts_with("sol_") {
                        found.push(if base == name {
                            Lint {
                                line: number,
                                severity: Severity::Warning,
                                message: format!(
                                    "`{word}` is not in this stage's `uses`, so it reads transparent"
                                ),
                            }
                        } else {
                            Lint {
                                line: number,
                                severity: Severity::Error,
                                message: format!(
                                    "`{word}` is declared only while `{base}` is in this stage's `uses`"
                                ),
                            }
                        });
                    }
                } else if !VOCABULARY.contains(&name) {
                    found.push(Lint {
                        line: number,
                        severity: Severity::Error,
                        message: format!("`{word}` is not part of the effect contract"),
                    });
                }
            }
        }
    }
    found
}

/// One line of a driver's compile log.
/// `tests::nvidia_mesa_and_angle_logs_map_to_the_users_line`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub string: Option<u32>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

fn number_then(text: &str) -> Option<(u32, &str)> {
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    Some((text.get(..digits)?.parse().ok()?, text.get(digits..)?))
}

/// One log line in any of the three forms in the wild, or `None`.
/// `tests::nvidia_mesa_and_angle_logs_map_to_the_users_line`.
fn parse_line(line: &str) -> Option<Diagnostic> {
    let line = line.trim();
    let body = line
        .strip_prefix("ERROR: ")
        .or_else(|| line.strip_prefix("WARNING: "))
        .unwrap_or(line);
    let (string, rest) = number_then(body)?;
    // NVIDIA: `1(12) : error C1008: …`
    if let Some(rest) = rest.strip_prefix('(') {
        let (number, rest) = number_then(rest)?;
        let message = rest
            .strip_prefix(')')?
            .trim_start_matches([' ', ':'])
            .to_owned();
        return Some(Diagnostic {
            string: Some(string),
            line: Some(number),
            column: None,
            message,
        });
    }
    // Mesa `1:12(5): error: …` and ANGLE `1:12: …`
    let (number, rest) = number_then(rest.strip_prefix(':')?)?;
    let (column, rest) = match rest.strip_prefix('(') {
        Some(inner) => {
            let (column, rest) = number_then(inner)?;
            (Some(column), rest.strip_prefix(')')?)
        }
        None => (None, rest),
    };
    let message = rest.trim_start_matches([' ', ':']).to_owned();
    Some(Diagnostic {
        string: Some(string),
        line: Some(number),
        column,
        message,
    })
}

/// A driver's log, line by line; a line no form matches is kept whole.
/// `tests::nvidia_mesa_and_angle_logs_map_to_the_users_line`.
pub fn compile_log(log: &str) -> Vec<Diagnostic> {
    log.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            parse_line(line).unwrap_or_else(|| Diagnostic {
                string: None,
                line: None,
                column: None,
                message: line.trim().to_owned(),
            })
        })
        .collect()
}

/// A `.frag` whose one error is on its own line 1, `sol_line_probe` being
/// declared nowhere: what tells a driver's `#line` rule.
/// `tests::a_drivers_line_rule_is_read_from_the_probe`.
pub const LINE_PROBE: &str = "vec4 sol_effect(vec2 uv) { return sol_line_probe; }\n";

/// [`LINE_PROBE`]'s three strings, against no params and no textures.
/// `tests::a_drivers_line_rule_is_read_from_the_probe`.
pub fn line_probe() -> Sources {
    let nothing = Signature {
        host: Host::Pass,
        params: Vec::new(),
        uses: Vec::new(),
        known: Vec::new(),
    };
    assemble(&nothing, LINE_PROBE)
}

/// How many lines lower than GLSL ES 1.00's `#line` rule a driver numbers
/// the user's file, read from its log of [`line_probe`]: 1 where it applies
/// GLSL ES 3.00's rule and calls the probe's line 1 line 0 (NVIDIA, wirecheck
/// case 12a), else 0, a log that names no line of string 1 included.
/// `tests::a_drivers_line_rule_is_read_from_the_probe`.
pub fn line_shift(probe_log: &str) -> u32 {
    let first = compile_log(probe_log)
        .into_iter()
        .find(|each| each.string == Some(1));
    match first.and_then(|each| each.line) {
        Some(0) => 1,
        _ => 0,
    }
}

/// FNV-1a 64 over `parts`, a 0xff byte after each so `["ab","c"]` and
/// `["a","bc"]` differ. Stable across builds, so a log line can name a version.
/// `tests::the_content_hash_is_stable_and_parts_do_not_run_together`.
pub fn content_hash(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.iter().copied().chain([0xff]) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{
        Host, LINE_PROBE, PASS_VERTEX, ParamKind, Signature, assemble, compile_log, content_hash,
        kind_of, line_probe, line_shift, lint, prelude,
    };
    use crate::spec::Severity;

    fn signature(uses: &[&str], known: &[&str]) -> Signature {
        Signature {
            host: Host::Pass,
            params: vec![
                ("passes".to_owned(), ParamKind::Int),
                ("offset".to_owned(), ParamKind::Float),
                ("tint".to_owned(), ParamKind::Vec4),
            ],
            uses: uses.iter().map(|each| (*each).to_owned()).collect(),
            known: known.iter().map(|each| (*each).to_owned()).collect(),
        }
    }

    fn has_line(text: &str, line: &str) -> bool {
        text.lines().any(|each| each.trim() == line)
    }

    /// **The prelude declares every param as its kind**, from `params`, so a
    /// `.frag` never declares one and cannot disagree with it.
    #[test]
    fn the_prelude_declares_every_param_as_its_kind() {
        let text = prelude(&signature(&[], &[]));
        assert!(has_line(&text, "uniform int p_passes;"), "{text}");
        assert!(has_line(&text, "uniform float p_offset;"));
        assert!(has_line(&text, "uniform vec4 p_tint;"));
        assert!(
            text.starts_with("#version 100\n"),
            "the version is the first line of the first string"
        );
    }

    /// **A name missing from `uses` reads transparent and has 0**: the
    /// function exists, so a `.frag` written for both cases compiles in both.
    #[test]
    fn a_name_missing_from_uses_reads_transparent_and_has_0() {
        let text = prelude(&signature(&["sharp"], &["sharp", "soft"]));
        assert!(has_line(&text, "#define SOL_HAS_sharp 1"), "{text}");
        assert!(has_line(&text, "#define SOL_HAS_soft 0"));
        assert!(text.contains("vec4 sol_soft(vec2 uv) { return vec4(0.0); }"));
        assert!(text.contains("uniform sampler2D sol_sharp_sampler;"));
        assert!(
            !text.contains("sol_soft_sampler"),
            "no sampler for a name not used"
        );
    }

    /// The rounded-box distance keeps both halves Task 21's clipped programs
    /// keep: the interior term and the radius clamp.
    #[test]
    fn the_prelude_sdf_keeps_the_interior_term_and_the_clamp() {
        let text = prelude(&signature(&[], &[]));
        assert!(
            text.contains("min(max(p.x, p.y), 0.0) + length(max(p, 0.0)) - r"),
            "{text}"
        );
        assert!(text.contains("min(picked, min(half_size.x, half_size.y))"));
    }

    /// **The user's file is its own source string**, byte for byte, so the
    /// driver's line numbers are the user's.
    #[test]
    fn the_users_file_is_its_own_source_string() {
        let user = "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv);\n}\n";
        let sources = assemble(&signature(&[], &[]), user);
        assert_eq!(sources.user, user);
        assert!(sources.epilogue.contains("sol_effect(v_uv)"));
        assert!(
            sources.prelude.ends_with("\n#line 0 1\n"),
            "the user's file is numbered from line 1 of string 1, even where the strings are joined"
        );
        assert!(
            sources.epilogue.starts_with("\n#line 0 2\n"),
            "a newline first, for a file with no final newline"
        );
        let warp = assemble(
            &Signature {
                host: Host::Warp,
                ..signature(&[], &[])
            },
            user,
        );
        assert!(
            warp.epilogue.contains("v_uvq.xy / v_uvq.z") && warp.epilogue.contains("* alpha"),
            "{}",
            warp.epilogue
        );
    }

    #[test]
    fn lint_names_an_undeclared_param_at_its_line() {
        let found = lint(
            &signature(&[], &[]),
            "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv) * p_ofset;\n}\n",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].line, found[0].severity), (2, Severity::Error));
        assert!(
            found[0].message.contains("p_ofset") && found[0].message.contains("p_offset"),
            "{found:?}"
        );
    }

    #[test]
    fn lint_refuses_a_uniform_the_engine_declares() {
        let found = lint(
            &signature(&[], &[]),
            "uniform float p_offset;\nvec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
        );
        assert!(
            found
                .iter()
                .any(|each| each.line == 1 && each.severity == Severity::Error),
            "{found:?}"
        );
    }

    #[test]
    fn lint_warns_about_a_known_name_missing_from_uses() {
        let found = lint(
            &signature(&["sharp"], &["sharp", "soft"]),
            "vec4 sol_effect(vec2 uv) { return sol_soft(uv); }\n",
        );
        assert!(
            found.iter().any(|each| each.severity == Severity::Warning
                && each.message.contains("sol_soft")),
            "{found:?}"
        );
        let unknown = lint(
            &signature(&[], &[]),
            "vec4 sol_effect(vec2 uv) { return sol_blurry(uv); }\n",
        );
        assert!(
            unknown.iter().any(|each| each.severity == Severity::Error
                && each.message.contains("sol_blurry")),
            "{unknown:?}"
        );
    }

    /// A used name's sampler and box are that name's, declared by the
    /// prelude: reading them is no warning. A name not in `uses` has neither
    /// declared, so reading one is an error, not "reads transparent".
    #[test]
    fn lint_reads_a_names_sampler_and_box_as_that_name() {
        let used = lint(
            &signature(&["sharp"], &["sharp", "soft"]),
            "vec4 sol_effect(vec2 uv) { return texture2D(sol_sharp_sampler, sol_sharp_box.xy + uv); }\n",
        );
        assert!(used.is_empty(), "{used:?}");
        let unused = lint(
            &signature(&["sharp"], &["sharp", "soft"]),
            "vec4 sol_effect(vec2 uv) { return texture2D(sol_soft_sampler, uv); }\n",
        );
        assert!(
            unused.iter().any(|each| each.severity == Severity::Error
                && each.message.contains("sol_soft_sampler")),
            "{unused:?}"
        );
    }

    /// Comments are not code: a `p_` in one is not a param read.
    #[test]
    fn lint_skips_comments() {
        assert!(
            lint(
                &signature(&[], &[]),
                "// p_nope\n/* sol_nope */ vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n"
            )
            .is_empty()
        );
    }

    /// **NVIDIA's, Mesa's and ANGLE's logs map to the user's line.**
    #[test]
    fn nvidia_mesa_and_angle_logs_map_to_the_users_line() {
        for log in [
            "1(12) : error C1008: undefined variable \"p_ofset\"",
            "1:12(5): error: `p_ofset' undeclared",
            "ERROR: 1:12: 'p_ofset' : undeclared identifier",
        ] {
            let found = compile_log(log);
            assert_eq!(
                (found[0].string, found[0].line),
                (Some(1), Some(12)),
                "{log}"
            );
        }
        let raw = compile_log("something no form matches");
        assert_eq!((raw[0].string, raw[0].line), (None, None));
        assert_eq!(raw[0].message, "something no form matches");
    }

    /// **A driver's `#line` rule is read from its log of the probe**: NVIDIA
    /// calls the line after `#line 0 1` line 0 (GLSL ES 3.00's rule; its own
    /// log, wirecheck case 12a), Mesa and ANGLE line 1 (GLSL ES 1.00's); a
    /// log naming no line of string 1, from a driver that ignored `#line`,
    /// shifts nothing.
    #[test]
    fn a_drivers_line_rule_is_read_from_the_probe() {
        assert_eq!(
            line_shift("1(0) : error C1503: undefined variable \"sol_line_probe\""),
            1
        );
        assert_eq!(line_shift("1:1(35): error: `sol_line_probe' undeclared"), 0);
        assert_eq!(
            line_shift("ERROR: 1:1: 'sol_line_probe' : undeclared identifier"),
            0
        );
        assert_eq!(
            line_shift("0:61(35): error: `sol_line_probe' undeclared"),
            0
        );
        assert_eq!(line_shift("something no form matches"), 0);
        let probe = line_probe();
        assert_eq!(probe.user, LINE_PROBE);
        assert_eq!(
            LINE_PROBE.lines().count(),
            1,
            "the probe's error is on line 1"
        );
        assert!(probe.prelude.ends_with("#line 0 1\n"));
    }

    /// A param's kind is its default's: a boolean is an `int` 0 or 1, and a
    /// word has no uniform.
    #[test]
    fn a_params_kind_is_its_defaults() {
        use crate::spec::Value;
        assert_eq!(kind_of(&Value::Number(0.5)), Some(ParamKind::Float));
        assert_eq!(kind_of(&Value::Int(3)), Some(ParamKind::Int));
        assert_eq!(kind_of(&Value::Bool(true)), Some(ParamKind::Int));
        assert_eq!(kind_of(&Value::Vec4([0.0; 4])), Some(ParamKind::Vec4));
        assert_eq!(kind_of(&Value::Word("soft".to_owned())), None);
    }

    /// A program's key changes with any of its three strings and with its
    /// vertex shader, and with nothing else.
    #[test]
    fn a_programs_key_is_its_three_strings_and_its_vertex_shader() {
        let user = "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n";
        let sources = assemble(&signature(&[], &[]), user);
        assert_eq!(
            sources.strings(),
            [sources.prelude.as_str(), user, sources.epilogue]
        );
        let key = sources.key(PASS_VERTEX);
        assert_eq!(key, assemble(&signature(&[], &[]), user).key(PASS_VERTEX));
        assert_ne!(key, sources.key("#version 100\nvoid main() {}\n"));
        let other = assemble(
            &signature(&[], &[]),
            "vec4 sol_effect(vec2 uv) { return vec4(1.0); }\n",
        );
        assert_ne!(key, other.key(PASS_VERTEX));
        assert_ne!(
            key,
            assemble(&signature(&["sharp"], &["sharp"]), user).key(PASS_VERTEX),
            "the prelude is in the key"
        );
        let warp = assemble(
            &Signature {
                host: Host::Warp,
                ..signature(&[], &[])
            },
            user,
        );
        assert_ne!(key, warp.key(PASS_VERTEX), "the epilogue is in the key");
    }

    #[test]
    fn the_content_hash_is_stable_and_parts_do_not_run_together() {
        assert_eq!(content_hash(&[b"ab", b"c"]), content_hash(&[b"ab", b"c"]));
        assert_ne!(content_hash(&[b"ab", b"c"]), content_hash(&[b"a", b"bc"]));
        assert_eq!(
            content_hash(&[]),
            0xcbf2_9ce4_8422_2325,
            "FNV-1a 64's offset basis"
        );
    }
}
