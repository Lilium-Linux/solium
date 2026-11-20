//! An effect's stages as an `effect.toml` writes them: the stages of
//! [`crate::stage`], each of which may run only `when` an expression of the
//! params is true and `repeat` a number of times they give, and `use`
//! another effect with params worked out from its own. Expanded into a plain
//! [`Stage`] list when the effect is loaded and again whenever a rule binds it
//! with other params, so nothing is decided per frame and the flattening,
//! memo and plans downstream are the same whatever wrote the stages.
//! `tests::blurs_stages_expand_as_its_lua_made_them`,
//! `tests::glass_uses_blur_only_when_it_softens`.

use crate::expr::Expr;
use crate::spec::{ParamSpec, Value};
use crate::stage::Stage;

/// The most stages an expansion makes, `use`d effects' not counted: a
/// `repeat` inside a `repeat` could otherwise ask for millions, and the
/// plan a list is flattened into runs at most 256 passes anyway.
/// `tests::an_expansion_past_its_limit_is_refused`.
const MOST_STAGES: usize = 1024;

/// The most times one `repeat` runs its stage.
const MOST_TIMES: f64 = 256.0;

/// How many times a stage runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Times {
    /// As many times as the expression gives: a whole number, 0 to 256.
    Count(Expr),
    /// Once for each number, with `p_<as_name>` set to it: a
    /// [`Stage::Repeat`].
    Over { over: Vec<f64>, as_name: String },
}

/// A value a `use` hands the effect it uses: as written, or worked out from
/// the user's params.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Literal(Value),
    Expr(Expr),
}

/// What a stage does.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A `pass`, `save` or `get`, which has nothing to work out.
    Fixed(Stage),
    Use {
        effect: String,
        params: Vec<(String, Arg)>,
    },
    /// A `state`, its body templates too.
    State {
        name: String,
        format: crate::stage::Format,
        scale: f64,
        depends: crate::stage::Depends,
        body: Vec<Template>,
    },
    /// Only `stages`: the stages in it, in its place, which is how one
    /// `when` or `repeat` covers several.
    Group(Vec<Template>),
}

/// One stage as written.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub kind: Kind,
    pub when: Option<Expr>,
    pub times: Option<Times>,
}

impl Template {
    /// A stage that always runs, once.
    #[must_use]
    pub fn once(kind: Kind) -> Self {
        Self {
            kind,
            when: None,
            times: None,
        }
    }
}

/// What an `effect.toml` works out from its params: its stages, and its
/// `reach` and `bleed` when it gave them as expressions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Described {
    pub stages: Vec<Template>,
    pub reach: Option<Expr>,
    pub bleed: Option<Expr>,
}

impl Described {
    /// Its stages for the bound `params`.
    pub fn stages(&self, params: &[(String, Value)]) -> Result<Vec<Stage>, String> {
        expand(&self.stages, params)
    }

    /// `reach` (`bleed` when `bleed`) for the bound `params`: `fixed` when
    /// it was written as a number, else its expression's value, which must be
    /// pixels, 0 or more. `tests::an_extent_is_worked_out_and_checked`.
    pub fn extent(
        &self,
        bleed: bool,
        fixed: f64,
        params: &[(String, Value)],
    ) -> Result<f64, String> {
        let (key, expr) = if bleed {
            ("bleed", &self.bleed)
        } else {
            ("reach", &self.reach)
        };
        let Some(expr) = expr else {
            return Ok(fixed);
        };
        let pixels = expr.eval(params);
        if pixels.is_finite() && pixels >= 0.0 {
            Ok(pixels)
        } else {
            Err(format!(
                "`{key}` ({}) gave {pixels}, not a number of pixels",
                expr.text()
            ))
        }
    }
}

/// Refused where an effect is loaded: an expression reading a param that is
/// four numbers, which is no number. `tests::an_expression_may_read_only_numbers`.
pub fn reads_numbers(expr: &Expr, params: &[(String, ParamSpec)]) -> Result<(), String> {
    for name in expr.params() {
        if let Some((
            _,
            ParamSpec {
                default: Value::Vec4(_) | Value::Word(_),
                ..
            },
        )) = params.iter().find(|(each, _)| each == name)
        {
            return Err(format!(
                "`{name}` is four numbers, and an expression reads only a number"
            ));
        }
    }
    Ok(())
}

/// `templates` for the bound `params`, as a plain stage list.
/// `tests::blurs_stages_expand_as_its_lua_made_them`.
pub fn expand(templates: &[Template], params: &[(String, Value)]) -> Result<Vec<Stage>, String> {
    let mut stages = Vec::new();
    let mut made = 0;
    expand_into(templates, params, &mut stages, &mut made)?;
    Ok(stages)
}

fn expand_into(
    templates: &[Template],
    params: &[(String, Value)],
    into: &mut Vec<Stage>,
    made: &mut usize,
) -> Result<(), String> {
    for (index, template) in templates.iter().enumerate() {
        let at = index + 1;
        expand_one(template, params, into, made)
            .map_err(|message| format!("stage {at}: {message}"))?;
    }
    Ok(())
}

fn expand_one(
    template: &Template,
    params: &[(String, Value)],
    into: &mut Vec<Stage>,
    made: &mut usize,
) -> Result<(), String> {
    if let Some(when) = &template.when {
        let value = when.eval(params);
        if value.is_nan() {
            return Err(format!("`when` ({}) gave no number", when.text()));
        }
        if value == 0.0 {
            return Ok(());
        }
    }
    match &template.times {
        None => kind(&template.kind, params, into, made),
        Some(Times::Count(count)) => {
            let times = count.eval(params);
            if !(times.fract() == 0.0 && (0.0..=MOST_TIMES).contains(&times)) {
                return Err(format!(
                    "`repeat` ({}) gave {times}, not a whole number of times from 0 to {MOST_TIMES}",
                    count.text()
                ));
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a whole number from 0 to 256, checked just here"
            )]
            for _ in 0..times as usize {
                kind(&template.kind, params, into, made)?;
            }
            Ok(())
        }
        Some(Times::Over { over, as_name }) => {
            let mut body = Vec::new();
            kind(&template.kind, params, &mut body, made)?;
            into.push(Stage::Repeat {
                over: over.clone(),
                as_name: as_name.clone(),
                body,
            });
            Ok(())
        }
    }
}

fn kind(
    kind: &Kind,
    params: &[(String, Value)],
    into: &mut Vec<Stage>,
    made: &mut usize,
) -> Result<(), String> {
    *made += 1;
    if *made > MOST_STAGES {
        return Err(format!("the stages expand to more than {MOST_STAGES}"));
    }
    match kind {
        Kind::Fixed(stage) => into.push(stage.clone()),
        Kind::Use {
            effect,
            params: args,
        } => {
            let mut given = Vec::with_capacity(args.len());
            for (name, arg) in args {
                let value = match arg {
                    Arg::Literal(value) => value.clone(),
                    Arg::Expr(expr) => {
                        let number = expr.eval(params);
                        if !number.is_finite() {
                            return Err(format!(
                                "`use`'s `{name}` ({}) gave {number}",
                                expr.text()
                            ));
                        }
                        Value::Number(number)
                    }
                };
                given.push((name.clone(), value));
            }
            into.push(Stage::Use {
                effect: effect.clone(),
                params: given,
            });
        }
        Kind::State {
            name,
            format,
            scale,
            depends,
            body,
        } => {
            let mut stages = Vec::new();
            expand_into(body, params, &mut stages, made)
                .map_err(|message| format!("in its `stages`, {message}"))?;
            into.push(Stage::State {
                name: name.clone(),
                format: *format,
                scale: *scale,
                depends: *depends,
                body: stages,
            });
        }
        Kind::Group(body) => expand_into(body, params, into, made)
            .map_err(|message| format!("in its `stages`, {message}"))?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Arg, Described, Kind, Template, Times, expand, reads_numbers};
    use crate::expr::Expr;
    use crate::spec::{ParamSpec, Value};
    use crate::stage::{Depends, Format, Stage};

    fn pass(frag: &str, scale: f64) -> Stage {
        Stage::Pass {
            frag: frag.to_owned(),
            scale,
            format: Format::Rgba8,
            uses: Vec::new(),
            input: None,
            per_part: false,
        }
    }

    fn expr(text: &str, names: &[&str]) -> Expr {
        Expr::parse(text, names).unwrap_or_else(|err| panic!("{text}: {err}"))
    }

    fn bound(values: &[(&str, Value)]) -> Vec<(String, Value)> {
        values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect()
    }

    /// The shipped blur's `effect.toml` stages, `repeat = "passes"` on a
    /// down pass and on an up pass, expand to what its `effect.lua`'s
    /// `stages(p)` made: `passes` halvings, then as many doublings.
    #[test]
    fn blurs_stages_expand_as_its_lua_made_them() {
        let names = ["offset", "passes"];
        let blur = [
            Template {
                kind: Kind::Fixed(pass("down.frag", 0.5)),
                when: None,
                times: Some(Times::Count(expr("passes", &names))),
            },
            Template {
                kind: Kind::Fixed(pass("up.frag", 2.0)),
                when: None,
                times: Some(Times::Count(expr("passes", &names))),
            },
        ];
        for passes in 1..=6 {
            let stages = expand(
                &blur,
                &bound(&[
                    ("offset", Value::Number(3.0)),
                    ("passes", Value::Int(passes)),
                ]),
            )
            .expect("expands");
            let mut lua = Vec::new();
            for _ in 0..passes {
                lua.push(pass("down.frag", 0.5));
            }
            for _ in 0..passes {
                lua.push(pass("up.frag", 2.0));
            }
            assert_eq!(stages, lua, "{passes} passes");
        }
    }

    /// Glass's stages: `use = "blur"` only `when` it softens, handing blur
    /// its own params by name, as its `effect.lua` did with an `if`.
    #[test]
    fn glass_uses_blur_only_when_it_softens() {
        let names = ["soften_offset", "soften_passes"];
        let glass = [
            Template::once(Kind::Fixed(Stage::Save("sharp".to_owned()))),
            Template {
                kind: Kind::Use {
                    effect: "blur".to_owned(),
                    params: vec![
                        (
                            "passes".to_owned(),
                            Arg::Expr(expr("soften_passes", &names)),
                        ),
                        (
                            "offset".to_owned(),
                            Arg::Expr(expr("soften_offset", &names)),
                        ),
                    ],
                },
                when: Some(expr("soften_passes >= 1", &names)),
                times: None,
            },
            Template::once(Kind::Fixed(Stage::Save("soft".to_owned()))),
        ];
        let at = |passes: i64| {
            expand(
                &glass,
                &bound(&[
                    ("soften_offset", Value::Number(6.0)),
                    ("soften_passes", Value::Int(passes)),
                ]),
            )
            .expect("expands")
        };
        assert_eq!(
            at(5),
            vec![
                Stage::Save("sharp".to_owned()),
                Stage::Use {
                    effect: "blur".to_owned(),
                    params: vec![
                        ("passes".to_owned(), Value::Number(5.0)),
                        ("offset".to_owned(), Value::Number(6.0)),
                    ],
                },
                Stage::Save("soft".to_owned()),
            ]
        );
        assert_eq!(
            at(0),
            vec![
                Stage::Save("sharp".to_owned()),
                Stage::Save("soft".to_owned())
            ],
            "clear glass: no blur"
        );
    }

    /// A count that is not whole, or past 256, is refused with the stage it
    /// is in; `over` makes a `Stage::Repeat`; a group repeats all it holds;
    /// a state's body is expanded too.
    #[test]
    fn repeats_groups_and_states_expand_and_bad_counts_are_refused() {
        let names = ["n"];
        let counted = |text: &str| {
            [Template {
                kind: Kind::Fixed(pass("a.frag", 1.0)),
                when: None,
                times: Some(Times::Count(expr(text, &names))),
            }]
        };
        let n = bound(&[("n", Value::Number(2.0))]);
        assert_eq!(expand(&counted("n"), &n).expect("expands").len(), 2);
        assert_eq!(expand(&counted("n - 2"), &n).expect("expands").len(), 0);
        for text in ["n / 4", "n * 200", "n - 3", "n / 0"] {
            let refused = expand(&counted(text), &n).expect_err(text);
            assert!(
                refused.starts_with("stage 1: `repeat`"),
                "{text}: {refused}"
            );
        }
        let over = [Template {
            kind: Kind::Fixed(pass("jump.frag", 1.0)),
            when: None,
            times: Some(Times::Over {
                over: vec![64.0, 32.0],
                as_name: "jump".to_owned(),
            }),
        }];
        assert_eq!(
            expand(&over, &n).expect("expands"),
            vec![Stage::Repeat {
                over: vec![64.0, 32.0],
                as_name: "jump".to_owned(),
                body: vec![pass("jump.frag", 1.0)],
            }]
        );
        let group = [Template {
            kind: Kind::Group(vec![
                Template::once(Kind::Fixed(pass("a.frag", 1.0))),
                Template::once(Kind::Fixed(pass("b.frag", 1.0))),
            ]),
            when: None,
            times: Some(Times::Count(expr("n", &names))),
        }];
        assert_eq!(
            expand(&group, &n).expect("expands"),
            vec![
                pass("a.frag", 1.0),
                pass("b.frag", 1.0),
                pass("a.frag", 1.0),
                pass("b.frag", 1.0)
            ]
        );
        let state = [Template::once(Kind::State {
            name: "field".to_owned(),
            format: Format::Rgba16f,
            scale: 0.5,
            depends: Depends::Shape,
            body: counted("n").to_vec(),
        })];
        let Stage::State { body, .. } = &expand(&state, &n).expect("expands")[0] else {
            panic!("a state");
        };
        assert_eq!(body.len(), 2);
        let broken = [Template::once(Kind::Group(counted("n / 4").to_vec()))];
        let refused = expand(&broken, &n).expect_err("half a pass");
        assert!(
            refused.starts_with("stage 1: in its `stages`, stage 1: `repeat`"),
            "{refused}"
        );
    }

    /// A `repeat` of a `repeat` cannot ask for millions of stages.
    #[test]
    fn an_expansion_past_its_limit_is_refused() {
        let inner = Template {
            kind: Kind::Fixed(pass("a.frag", 1.0)),
            when: None,
            times: Some(Times::Count(expr("256", &[]))),
        };
        let outer = [Template {
            kind: Kind::Group(vec![inner]),
            when: None,
            times: Some(Times::Count(expr("256", &[]))),
        }];
        let refused = expand(&outer, &[]).expect_err("65 536 stages");
        assert!(refused.contains("more than 1024"), "{refused}");
    }

    #[test]
    fn an_extent_is_worked_out_and_checked() {
        let names = ["offset", "passes"];
        let described = Described {
            reach: Some(expr("ceil(offset * 2 ^ (passes + 1))", &names)),
            bleed: Some(expr("offset - 10", &names)),
            ..Described::default()
        };
        let params = bound(&[("offset", Value::Number(3.0)), ("passes", Value::Int(3))]);
        assert!((described.extent(false, 0.0, &params).expect("reach") - 48.0).abs() < 1e-12);
        let refused = described.extent(true, 0.0, &params).expect_err("-7 pixels");
        assert!(refused.contains("`bleed`"), "{refused}");
        let fixed = Described::default();
        assert!((fixed.extent(true, 5.0, &params).expect("fixed") - 5.0).abs() < 1e-12);
    }

    #[test]
    fn an_expression_may_read_only_numbers() {
        let params = vec![
            (
                "tint".to_owned(),
                ParamSpec {
                    default: Value::Vec4([0.0; 4]),
                    min: None,
                    max: None,
                },
            ),
            (
                "on".to_owned(),
                ParamSpec {
                    default: Value::Bool(true),
                    min: None,
                    max: None,
                },
            ),
        ];
        let names = ["tint", "on"];
        assert!(reads_numbers(&expr("on + 1", &names), &params).is_ok());
        let refused = reads_numbers(&expr("tint > 0", &names), &params).expect_err("a colour");
        assert!(refused.contains("`tint`"), "{refused}");
    }
}
