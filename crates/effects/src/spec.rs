//! What an `effect.lua` said, as plain data: no Lua here, so the schema is
//! testable and the browser preview can read it (\[16\] §2, Ruling 5).

/// The keys an `effect.lua` may return. Anything else is refused by name.
pub const KEYS: [&str; 13] = [
    "api", "inputs", "params", "reach", "bleed", "stages", "frag", "pixels", "fallback",
    "duration", "easing", "grid", "mesh",
];

/// Names a param may not have: what a rule or `effects.on` says beside params
/// (`reach` and `bleed` there override the file's, Ruling 5).
pub const RESERVED: [&str; 15] = [
    "source",
    "mask",
    "keep",
    "effect",
    "geometry",
    "pixels",
    "decoration",
    "duration",
    "easing",
    "motion",
    "from",
    "to",
    "axis",
    "reach",
    "bleed",
];

/// The inputs the engine has, besides `state:<name>`.
pub const INPUTS: [&str; 4] = ["self", "backdrop", "shape", "old"];

/// How bad a problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A param's value. `Bool` is an `int` 0 or 1 in GLSL; `Word` has no uniform.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Int(i64),
    Bool(bool),
    Vec4([f64; 4]),
    Word(String),
}

/// A param as declared: its default, whose kind is the param's, and its range.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub default: Value,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// A cheaper rung for the ladder (X2.7): param overrides, or another effect.
#[derive(Clone, Debug, PartialEq)]
pub enum Rung {
    Params(Vec<(String, Value)>),
    Effect(String),
}

/// A geometry effect's grid: one that turns with the axis, or a fixed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridSpec {
    Turning { along: u32, across: u32 },
    Fixed { cols: u32, rows: u32 },
}

/// `reach` or `bleed`: a number, or a function of the params called at bind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Extent {
    Fixed(f64),
    Function,
}

impl Default for Extent {
    fn default() -> Self {
        Self::Fixed(0.0)
    }
}

/// Whether `stages` was given, and as what.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Given {
    #[default]
    Absent,
    List,
    Function,
}

/// An `effect.lua`'s table, read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectSpec {
    pub api: u32,
    pub inputs: Vec<String>,
    pub params: Vec<(String, ParamSpec)>,
    pub fallback: Vec<Rung>,
    pub frag: Option<String>,
    pub pixels: Option<String>,
    pub duration: Option<f64>,
    pub easing: Option<String>,
    pub grid: Option<GridSpec>,
    pub stages: Given,
    pub mesh: bool,
    pub reach: Extent,
    pub bleed: Extent,
}

/// A name GLSL can carry as `p_<name>` or `sol_<name>`: lower-case letters,
/// digits and `_`, not starting with a digit.
pub(crate) fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => Some(*number),
        #[expect(clippy::cast_precision_loss, reason = "a param, far below 2^53")]
        Value::Int(number) => Some(*number as f64),
        _ => None,
    }
}

impl EffectSpec {
    /// What the schema cannot say by shape alone. `tests::an_effect_says_what_it_draws_once`,
    /// `tests::names_inputs_and_defaults_are_checked`.
    pub fn check(&self) -> Result<(), String> {
        if self.api != 1 {
            return Err(format!(
                "`api` is {}, and this Solium reads api 1",
                self.api
            ));
        }
        if self.frag.is_none()
            && self.stages == Given::Absent
            && !self.mesh
            && self.pixels.is_none()
        {
            return Err(
                "the effect draws nothing: give `frag`, `stages`, `mesh` or `pixels`".to_owned(),
            );
        }
        if self.frag.is_some() && self.stages != Given::Absent {
            return Err(
                "`frag` is the one-pass shorthand for `stages`; give one of them".to_owned(),
            );
        }
        if self.frag.is_some() && self.pixels.is_some() {
            return Err(
                "`pixels` names another effect's frag; give it or `frag`, not both".to_owned(),
            );
        }
        for input in &self.inputs {
            if input.starts_with("image:") {
                // [16] §2's `image:<file>`: deferred, with its seam here and
                // in `run::Inputs::textures` (*What FX2 does not build*).
                // `tests::names_inputs_and_defaults_are_checked`.
                return Err(format!(
                    "input `{input}`: `image:` inputs arrive with X1.3b (FX3)"
                ));
            }
            let state = input.strip_prefix("state:").is_some_and(is_identifier);
            if !state && !INPUTS.contains(&input.as_str()) {
                return Err(format!(
                    "`{input}` is not an input: the inputs are {}, and `state:<name>`",
                    INPUTS.join(", ")
                ));
            }
        }
        for (name, param) in &self.params {
            if RESERVED.contains(&name.as_str()) {
                return Err(format!(
                    "a param may not be called `{name}`: a rule says that beside the params"
                ));
            }
            if !is_identifier(name) {
                return Err(format!(
                    "param `{name}`: a name is lower-case letters, digits and `_`"
                ));
            }
            if let Some(default) = number(&param.default)
                && (param.min.is_some_and(|min| default < min)
                    || param.max.is_some_and(|max| default > max))
            {
                return Err(format!(
                    "param `{name}`'s default is outside its own `min` and `max`"
                ));
            }
        }
        for (key, extent) in [("reach", self.reach), ("bleed", self.bleed)] {
            if let Extent::Fixed(value) = extent
                && !(value.is_finite() && value >= 0.0)
            {
                return Err(format!("`{key}` must be a number of pixels, 0 or more"));
            }
        }
        Ok(())
    }
}

/// The params bound with `overrides`: every param in declared order, the
/// override's value where one is given, clamped into range with a warning.
/// `tests::binding_refuses_an_unknown_param_and_clamps_out_of_range`,
/// `tests::binding_converts_what_can_be_converted_and_refuses_the_rest`.
#[expect(
    clippy::type_complexity,
    reason = "the bound params and the warnings, read once by the host"
)]
pub fn bind(
    params: &[(String, ParamSpec)],
    overrides: &[(String, Value)],
) -> Result<(Vec<(String, Value)>, Vec<String>), String> {
    let names: Vec<&str> = params.iter().map(|(name, _)| name.as_str()).collect();
    for (name, _) in overrides {
        if !names.contains(&name.as_str()) {
            let meant = nearest(name, &names)
                .map(|meant| format!("; did you mean `{meant}`?"))
                .unwrap_or_default();
            return Err(format!("the effect has no param `{name}`{meant}"));
        }
    }
    let mut warnings = Vec::new();
    let mut bound = Vec::with_capacity(params.len());
    for (name, param) in params {
        let given = overrides
            .iter()
            .rev()
            .find(|(each, _)| each == name)
            .map(|(_, value)| value);
        let value = match (given, &param.default) {
            (None, default) => default.clone(),
            (Some(Value::Int(int)), Value::Number(_)) => {
                #[expect(clippy::cast_precision_loss, reason = "a param, far below 2^53")]
                let number = *int as f64;
                Value::Number(number)
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a whole number checked just here"
            )]
            (Some(Value::Number(number)), Value::Int(_)) if number.fract() == 0.0 => {
                Value::Int(*number as i64)
            }
            (Some(given), default)
                if std::mem::discriminant(given) == std::mem::discriminant(default) =>
            {
                given.clone()
            }
            (Some(_), _) => return Err(format!("param `{name}` is not that kind of value")),
        };
        let value = match number(&value) {
            Some(at) => {
                let clamped = at
                    .max(param.min.unwrap_or(f64::MIN))
                    .min(param.max.unwrap_or(f64::MAX));
                if (clamped - at).abs() > 0.0 {
                    warnings.push(format!(
                        "param `{name}` = {at} is out of range; using {clamped}"
                    ));
                }
                match value {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "clamped into an int param's own range"
                    )]
                    Value::Int(_) => Value::Int(clamped.round() as i64),
                    _ => Value::Number(clamped),
                }
            }
            None => value,
        };
        bound.push((name.clone(), value));
    }
    Ok((bound, warnings))
}

/// The word in `among` within two edits of `word`, if one is.
/// `tests::a_near_miss_is_suggested`.
pub fn nearest<'a>(word: &str, among: &[&'a str]) -> Option<&'a str> {
    fn distance(a: &str, b: &str) -> usize {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut previous = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let here = row[j + 1];
                row[j + 1] = (previous + usize::from(ca != *cb))
                    .min(row[j] + 1)
                    .min(here + 1);
                previous = here;
            }
        }
        row[b.len()]
    }
    among
        .iter()
        .copied()
        .map(|each| (distance(word, each), each))
        .filter(|(edits, _)| *edits <= 2)
        .min()
        .map(|(_, each)| each)
}

#[cfg(test)]
mod tests {
    use super::{EffectSpec, Extent, Given, ParamSpec, Value, bind, nearest};

    fn param(default: Value, min: Option<f64>, max: Option<f64>) -> ParamSpec {
        ParamSpec { default, min, max }
    }

    fn passes_and_offset() -> Vec<(String, ParamSpec)> {
        vec![
            (
                "passes".to_owned(),
                param(Value::Int(3), Some(1.0), Some(6.0)),
            ),
            (
                "offset".to_owned(),
                param(Value::Number(3.0), Some(0.0), None),
            ),
        ]
    }

    /// **Binding refuses an unknown param and clamps one out of range**, with
    /// a warning that names it: `passes = 9` is 6, `pases = 2` is a typo.
    #[test]
    fn binding_refuses_an_unknown_param_and_clamps_out_of_range() {
        let (bound, warnings) = bind(
            &passes_and_offset(),
            &[("passes".to_owned(), Value::Int(9))],
        )
        .expect("binds");
        assert_eq!(bound[0], ("passes".to_owned(), Value::Int(6)));
        assert_eq!(
            bound[1],
            ("offset".to_owned(), Value::Number(3.0)),
            "the default"
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("passes"), "{warnings:?}");
        let refused =
            bind(&passes_and_offset(), &[("pases".to_owned(), Value::Int(2))]).expect_err("a typo");
        assert!(
            refused.contains("pases") && refused.contains("passes"),
            "names it and suggests: {refused}"
        );
    }

    /// A whole number is a fine `float`; a fraction is not a fine `int`; a word
    /// is no number at all.
    #[test]
    fn binding_converts_what_can_be_converted_and_refuses_the_rest() {
        let (bound, _) = bind(
            &passes_and_offset(),
            &[("offset".to_owned(), Value::Int(2))],
        )
        .expect("binds");
        assert_eq!(bound[1], ("offset".to_owned(), Value::Number(2.0)));
        assert!(
            bind(
                &passes_and_offset(),
                &[("passes".to_owned(), Value::Number(2.5))]
            )
            .is_err()
        );
        assert!(
            bind(
                &passes_and_offset(),
                &[("offset".to_owned(), Value::Word("lots".to_owned()))]
            )
            .is_err()
        );
    }

    fn minimal() -> EffectSpec {
        EffectSpec {
            api: 1,
            frag: Some("effect.frag".to_owned()),
            ..EffectSpec::default()
        }
    }

    /// An effect must do something, and say it once: a `frag` or `stages`,
    /// not both; a `mesh`; or `pixels` naming another effect's frag.
    #[test]
    fn an_effect_says_what_it_draws_once() {
        assert!(minimal().check().is_ok());
        assert!(
            EffectSpec {
                frag: None,
                ..minimal()
            }
            .check()
            .is_err(),
            "nothing to draw"
        );
        assert!(
            EffectSpec {
                stages: Given::List,
                ..minimal()
            }
            .check()
            .is_err(),
            "frag and stages"
        );
        assert!(
            EffectSpec {
                pixels: Some("fade".to_owned()),
                ..minimal()
            }
            .check()
            .is_err(),
            "frag and pixels"
        );
        assert!(
            EffectSpec {
                frag: None,
                mesh: true,
                pixels: Some("fade".to_owned()),
                ..minimal()
            }
            .check()
            .is_ok()
        );
        assert!(
            EffectSpec {
                api: 2,
                ..minimal()
            }
            .check()
            .is_err()
        );
    }

    /// A param may not take a name a rule uses beside params, an input must be
    /// one the engine has, and a default must sit inside its own range.
    #[test]
    fn names_inputs_and_defaults_are_checked() {
        let with = |name: &str| EffectSpec {
            params: vec![(name.to_owned(), param(Value::Number(1.0), None, None))],
            ..minimal()
        };
        assert!(with("source").check().is_err());
        assert!(
            with("bleed").check().is_err(),
            "a rule's extent override, Ruling 5"
        );
        assert!(with("Passes").check().is_err(), "not a GLSL-safe name");
        assert!(with("passes").check().is_ok());
        assert!(
            EffectSpec {
                inputs: vec!["backdrop".to_owned(), "state:distance".to_owned()],
                ..minimal()
            }
            .check()
            .is_ok()
        );
        let image = EffectSpec {
            inputs: vec!["image:grain.png".to_owned()],
            ..minimal()
        }
        .check()
        .expect_err("not yet");
        assert!(
            image.contains("X1.3b"),
            "names the item that brings it: {image}"
        );
        assert!(
            EffectSpec {
                inputs: vec!["selfie".to_owned()],
                ..minimal()
            }
            .check()
            .is_err()
        );
        let low = EffectSpec {
            params: vec![("n".to_owned(), param(Value::Int(0), Some(1.0), None))],
            ..minimal()
        };
        assert!(low.check().is_err(), "a default below its own min");
        assert!(
            EffectSpec {
                reach: Extent::Fixed(-1.0),
                ..minimal()
            }
            .check()
            .is_err()
        );
    }

    #[test]
    fn a_near_miss_is_suggested() {
        assert_eq!(nearest("stagse", &super::KEYS), Some("stages"));
        assert_eq!(nearest("zzzzzz", &super::KEYS), None);
    }
}
