//! An `effect.toml`, read: what an effect is, as data, with no Lua and no
//! code of the effect's run on the CPU. Its params, inputs and passes; its
//! `reach`, `bleed`, and each stage's `repeat` and `when` as expressions of
//! the params (`solium_effects::expr`), worked out by the compositor at load
//! and at bind. A problem names the line it is on.
//! `tests::the_shipped_folders_read`,
//! `tests::a_problem_is_at_its_line_and_names_what_was_meant`.
//!
//! Also a pane style's `effects.toml`, its rules, read into the same
//! [`Doc`] the window manager's rules arrive as ([`doc`]).

use std::path::Path;

use solium_effects::describe::{Arg, Described, Kind, Template, Times, reads_numbers};
use solium_effects::expr::Expr;
use solium_effects::spec::{self, EffectSpec, Extent, Given, GridSpec, ParamSpec, Rung, Value};
use solium_effects::stage::{Depends, Format, Stage};

use super::host::Problem;
use crate::script::{Doc, DocTable};

/// The api an `effect.toml` is written against.
pub(crate) const API: u32 = 2;

/// How deep a file's tables and lists may nest: far past any effect, and
/// short of what reading it could overflow the stack with.
const MOST_DEPTH: usize = 32;

/// A value of the file and the byte it starts at.
#[derive(Clone, Debug)]
struct Item {
    at: usize,
    value: Toml,
}

#[derive(Clone, Debug)]
enum Toml {
    Text(String),
    Int(i64),
    Num(f64),
    Bool(bool),
    List(Vec<Item>),
    Table(Vec<(String, Item)>),
    Date,
}

impl Toml {
    fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "a string",
            Self::Int(_) => "a whole number",
            Self::Num(_) => "a number",
            Self::Bool(_) => "true or false",
            Self::List(_) => "a list",
            Self::Table(_) => "a table",
            Self::Date => "a date",
        }
    }
}

/// What is wrong, and the byte it is at when that is known.
type Refused = (Option<usize>, String);

fn at(item: &Item, message: impl Into<String>) -> Refused {
    (Some(item.at), message.into())
}

/// The line `at` is on, from 1.
fn line_of(text: &str, at: usize) -> u32 {
    let before = text.get(..at).unwrap_or(text);
    u32::try_from(before.bytes().filter(|byte| *byte == b'\n').count() + 1).unwrap_or(u32::MAX)
}

fn convert(value: toml::Spanned<toml::de::DeValue<'_>>, depth: usize) -> Result<Item, Refused> {
    use toml::de::DeValue;
    let at = value.span().start;
    if depth > MOST_DEPTH {
        return Err((Some(at), format!("nested more than {MOST_DEPTH} deep")));
    }
    let value = match value.into_inner() {
        DeValue::String(text) => Toml::Text(text.into_owned()),
        DeValue::Integer(int) => {
            let digits = int.as_str().replace('_', "");
            Toml::Int(
                i64::from_str_radix(&digits, int.radix())
                    .map_err(|_| (Some(at), format!("{int} is too large a number")))?,
            )
        }
        DeValue::Float(float) => Toml::Num(
            float
                .as_str()
                .replace('_', "")
                .parse::<f64>()
                .map_err(|_| (Some(at), format!("{float} is not a number")))?,
        ),
        DeValue::Boolean(yes) => Toml::Bool(yes),
        DeValue::Datetime(_) => Toml::Date,
        DeValue::Array(items) => Toml::List(
            items
                .into_iter()
                .map(|item| convert(item, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        DeValue::Table(table) => Toml::Table(table_of(table, depth + 1)?),
    };
    Ok(Item { at, value })
}

fn table_of(table: toml::de::DeTable<'_>, depth: usize) -> Result<Vec<(String, Item)>, Refused> {
    table
        .into_iter()
        .map(|(key, value)| Ok((key.into_inner().into_owned(), convert(value, depth)?)))
        .collect()
}

/// The file's top table, or why it is not TOML.
fn parse(text: &str) -> Result<Vec<(String, Item)>, Refused> {
    let table = toml::de::DeTable::parse(text).map_err(|err| {
        (
            err.span().map(|span| span.start),
            format!("not TOML: {}", err.message()),
        )
    })?;
    table_of(table.into_inner(), 0)
}

/// Read `file`, the effect `effect`'s `effect.toml`.
/// `tests::the_shipped_folders_read`.
pub(crate) fn read(effect: &str, file: &Path) -> Result<(EffectSpec, Described), Problem> {
    let text = std::fs::read_to_string(file)
        .map_err(|err| Problem::error(effect, file, None, format!("cannot read it: {err}")))?;
    read_text(&text).map_err(|(byte, message)| {
        Problem::error(effect, file, byte.map(|byte| line_of(&text, byte)), message)
    })
}

/// [`read`], from the file's text.
fn read_text(text: &str) -> Result<(EffectSpec, Described), Refused> {
    let top = parse(text)?;
    let mut spec = EffectSpec::default();
    let mut described = Described::default();
    // The params first: every expression after them is read against their
    // names.
    if let Some((_, params)) = top.iter().find(|(key, _)| key == "params") {
        spec.params = read_params(params)?;
    }
    let names: Vec<&str> = spec.params.iter().map(|(name, _)| name.as_str()).collect();
    let expression = |item: &Item, what: &str| -> Result<Expr, Refused> {
        let Toml::Text(text) = &item.value else {
            return Err(at(item, format!("{what} is an expression, a string")));
        };
        let expr =
            Expr::parse(text, &names).map_err(|message| at(item, format!("{what}: {message}")))?;
        reads_numbers(&expr, &spec.params)
            .map_err(|message| at(item, format!("{what}: {message}")))?;
        Ok(expr)
    };
    let mut api = None;
    for (key, item) in &top {
        match key.as_str() {
            "params" => {}
            "api" => {
                api = Some(item);
                let Toml::Int(number) = item.value else {
                    return Err(at(item, "`api` is a whole number"));
                };
                if number != i64::from(API) {
                    return Err(at(
                        item,
                        format!("`api` is {number}, and an effect.toml is api {API}"),
                    ));
                }
                spec.api = API;
            }
            "inputs" => spec.inputs = words(item, "inputs")?,
            "reach" | "bleed" => {
                let (extent, expr) = match &item.value {
                    Toml::Text(_) => (
                        Extent::Function,
                        Some(expression(item, &format!("`{key}`"))?),
                    ),
                    _ => (Extent::Fixed(number(item, &format!("`{key}`"))?), None),
                };
                if key == "reach" {
                    spec.reach = extent;
                    described.reach = expr;
                } else {
                    spec.bleed = extent;
                    described.bleed = expr;
                }
            }
            "stages" => {
                let Toml::List(list) = &item.value else {
                    return Err(at(item, "`stages` is a list of stages: [[stages]] tables"));
                };
                described.stages = stages(list, &expression)?;
                spec.stages = Given::List;
            }
            "frag" => spec.frag = Some(word(item, "frag")?),
            "pixels" => spec.pixels = Some(word(item, "pixels")?),
            "fallback" => spec.fallback = rungs(item)?,
            "duration" => spec.duration = Some(number(item, "`duration`, in milliseconds,")?),
            "easing" => {
                let name = word(item, "easing")?;
                if solium_animation::Curve::from_name(&name).is_none() {
                    return Err(at(
                        item,
                        format!("`easing = \"{name}\"` is not a curve this Solium has"),
                    ));
                }
                spec.easing = Some(name);
            }
            "grid" => spec.grid = Some(grid(item)?),
            "mesh" => spec.mesh = Some(word(item, "mesh")?),
            other => {
                let meant = spec::nearest(other, &spec::KEYS)
                    .map(|meant| format!("; did you mean `{meant}`?"))
                    .unwrap_or_default();
                return Err(at(
                    item,
                    format!("`{other}` is not a key of an effect.toml{meant}"),
                ));
            }
        }
    }
    if api.is_none() {
        return Err((
            None,
            format!("an effect.toml says its api first: api = {API}"),
        ));
    }
    spec.check().map_err(|message| (None, message))?;
    // `frag` is the shorthand for one pass of it (`check` refuses it beside
    // `stages`).
    if let Some(frag) = &spec.frag {
        described.stages = vec![Template::once(Kind::Fixed(Stage::Pass {
            frag: frag.clone(),
            scale: 1.0,
            format: Format::Rgba8,
            uses: Vec::new(),
            input: None,
            per_part: false,
        }))];
    }
    Ok((spec, described))
}

fn word(item: &Item, key: &str) -> Result<String, Refused> {
    match &item.value {
        Toml::Text(text) => Ok(text.clone()),
        other => Err(at(
            item,
            format!("`{key}` is a string, not {}", other.kind()),
        )),
    }
}

fn words(item: &Item, key: &str) -> Result<Vec<String>, Refused> {
    let Toml::List(list) = &item.value else {
        return Err(at(item, format!("`{key}` is a list of names")));
    };
    list.iter().map(|each| word(each, key)).collect()
}

/// A finite number, whole or not.
fn number(item: &Item, what: &str) -> Result<f64, Refused> {
    match item.value {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a number from a file, far below 2^53"
        )]
        Toml::Int(int) => Ok(int as f64),
        Toml::Num(number) if number.is_finite() => Ok(number),
        _ => Err(at(item, format!("{what} is a number"))),
    }
}

/// A value as a literal gives it: a number, a whole number, `true` or
/// `false`, or four numbers. No words: nothing in an effect.toml reads one.
fn literal(item: &Item, what: &str) -> Result<Value, Refused> {
    match &item.value {
        Toml::Int(int) => Ok(Value::Int(*int)),
        Toml::Num(number) if number.is_finite() => Ok(Value::Number(*number)),
        Toml::Bool(yes) => Ok(Value::Bool(*yes)),
        Toml::List(four) => {
            let numbers = four
                .iter()
                .map(|each| number(each, what))
                .collect::<Result<Vec<f64>, _>>()?;
            let [a, b, c, d] = numbers[..] else {
                return Err(at(item, format!("{what}: a list is four numbers")));
            };
            Ok(Value::Vec4([a, b, c, d]))
        }
        _ => Err(at(
            item,
            format!("{what} is a number, true or false, or four numbers"),
        )),
    }
}

/// The keys a param's table may have.
const PARAM_KEYS: [&str; 4] = ["default", "min", "max", "int"];

/// `[params]`: each `name = <default>`, or `name = { default =, min =, max
/// =, int = }`; sorted by name, so their uniform order is the file's no
/// matter how it is laid out. `tests::params_are_read_with_their_kinds_and_ranges`.
fn read_params(item: &Item) -> Result<Vec<(String, ParamSpec)>, Refused> {
    let Toml::Table(table) = &item.value else {
        return Err(at(
            item,
            "`params` is a table: [params], then name = { default = … }",
        ));
    };
    let mut read = Vec::with_capacity(table.len());
    for (name, param) in table {
        let what = format!("param `{name}`");
        let (default, int, min, max) = match &param.value {
            Toml::Table(keys) => {
                let mut default = None;
                let (mut int, mut min, mut max) = (false, None, None);
                for (key, value) in keys {
                    match key.as_str() {
                        "default" => default = Some(value),
                        "int" => {
                            let Toml::Bool(yes) = value.value else {
                                return Err(at(value, format!("{what}: `int` is true or false")));
                            };
                            int = yes;
                        }
                        "min" => min = Some(number(value, &format!("{what}'s `min`"))?),
                        "max" => max = Some(number(value, &format!("{what}'s `max`"))?),
                        other => {
                            let meant = spec::nearest(other, &PARAM_KEYS)
                                .map(|meant| format!("; did you mean `{meant}`?"))
                                .unwrap_or_default();
                            return Err(at(
                                value,
                                format!("{what}: `{other}` is not default, min, max or int{meant}"),
                            ));
                        }
                    }
                }
                let Some(default) = default else {
                    return Err(at(
                        param,
                        format!("{what} has no default: {name} = {{ default = … }}"),
                    ));
                };
                (default, int, min, max)
            }
            _ => (param, false, None, None),
        };
        let value = match (&default.value, int) {
            (Toml::Int(whole), true) => Value::Int(*whole),
            #[expect(clippy::cast_precision_loss, reason = "a param, far below 2^53")]
            (Toml::Int(whole), false) => Value::Number(*whole as f64),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a whole number checked just here"
            )]
            (Toml::Num(number), true) if number.fract() == 0.0 => Value::Int(*number as i64),
            (Toml::Num(_), true) => {
                return Err(at(
                    default,
                    format!("{what}: an `int` param's default is a whole number"),
                ));
            }
            (Toml::Text(_), _) => {
                return Err(at(
                    default,
                    format!(
                        "{what}: a param is a number, true or false, or four numbers; no expression or shader reads a word"
                    ),
                ));
            }
            _ => literal(default, &what)?,
        };
        read.push((
            name.clone(),
            ParamSpec {
                default: value,
                min,
                max,
            },
        ));
    }
    read.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(read)
}

/// `fallback`: rungs, each a table of params or another effect's name.
fn rungs(item: &Item) -> Result<Vec<Rung>, Refused> {
    const SHAPE: &str =
        "`fallback` is a list of rungs, each { <param> = <value>, … } or an effect's name";
    let Toml::List(list) = &item.value else {
        return Err(at(item, SHAPE));
    };
    list.iter()
        .map(|rung| match &rung.value {
            Toml::Text(name) => Ok(Rung::Effect(name.clone())),
            Toml::Table(params) => {
                let mut read = params
                    .iter()
                    .map(|(name, value)| {
                        Ok((
                            name.clone(),
                            literal(value, &format!("`fallback`'s `{name}`"))?,
                        ))
                    })
                    .collect::<Result<Vec<_>, Refused>>()?;
                read.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(Rung::Params(read))
            }
            _ => Err(at(rung, SHAPE)),
        })
        .collect()
}

/// `{ along = a, across = b }`, which turns with the axis, or `[cols, rows]`.
fn grid(item: &Item) -> Result<GridSpec, Refused> {
    const SHAPE: &str =
        "`grid` is { along = <n>, across = <n> } or [<cols>, <rows>], each 1 or more";
    let count = |item: &Item| match item.value {
        Toml::Int(n) if (1..=i64::from(u32::MAX)).contains(&n) => {
            u32::try_from(n).map_err(|_| at(item, SHAPE))
        }
        _ => Err(at(item, SHAPE)),
    };
    match &item.value {
        Toml::Table(keys) if keys.len() == 2 => {
            let get = |name: &str| {
                keys.iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value)
                    .ok_or_else(|| at(item, SHAPE))
            };
            Ok(GridSpec::Turning {
                along: count(get("along")?)?,
                across: count(get("across")?)?,
            })
        }
        Toml::List(pair) if pair.len() == 2 => Ok(GridSpec::Fixed {
            cols: count(&pair[0])?,
            rows: count(&pair[1])?,
        }),
        _ => Err(at(item, SHAPE)),
    }
}

/// The kinds of stage, each a key naming its file, its name or its effect;
/// a stage with none of them and `stages` is a group.
const KINDS: [&str; 5] = ["pass", "save", "get", "use", "state"];
/// What every stage may say beside its kind.
const MODIFIERS: [&str; 2] = ["when", "repeat"];
const PASS_KEYS: [&str; 5] = ["scale", "format", "uses", "input", "per_part"];
const USE_KEYS: [&str; 1] = ["params"];
const STATE_KEYS: [&str; 4] = ["depends", "format", "scale", "stages"];
const GROUP_KEYS: [&str; 1] = ["stages"];

fn stages(
    list: &[Item],
    expression: &dyn Fn(&Item, &str) -> Result<Expr, Refused>,
) -> Result<Vec<Template>, Refused> {
    list.iter()
        .enumerate()
        .map(|(index, item)| {
            stage(item, expression).map_err(|(byte, message)| {
                (
                    byte.or(Some(item.at)),
                    format!("stage {}: {message}", index + 1),
                )
            })
        })
        .collect()
}

fn stage(
    item: &Item,
    expression: &dyn Fn(&Item, &str) -> Result<Expr, Refused>,
) -> Result<Template, Refused> {
    let Toml::Table(keys) = &item.value else {
        return Err(at(
            item,
            "a stage is a table, such as { pass = \"effect.frag\" }",
        ));
    };
    let get = |name: &str| {
        keys.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    };
    let kinds: Vec<&str> = KINDS
        .iter()
        .copied()
        .filter(|kind| get(kind).is_some())
        .collect();
    let (kind, allowed): (&str, &[&str]) = match kinds.as_slice() {
        [] if get("stages").is_some() => ("stages", &GROUP_KEYS),
        [] => {
            return Err(at(
                item,
                "a stage says its kind: pass = \"<file>.frag\", save, get, use = \"<effect>\" or state, or only stages = [ … ]",
            ));
        }
        ["pass"] => ("pass", &PASS_KEYS),
        ["use"] => ("use", &USE_KEYS),
        ["state"] => ("state", &STATE_KEYS),
        [single] => (*single, &[]),
        [first, second, ..] => {
            return Err(at(
                item,
                format!("a stage is one kind, not both `{first}` and `{second}`"),
            ));
        }
    };
    for (key, value) in keys {
        if key == kind || allowed.contains(&key.as_str()) || MODIFIERS.contains(&key.as_str()) {
            continue;
        }
        let mut known: Vec<&str> = allowed.to_vec();
        known.extend(MODIFIERS);
        let meant = spec::nearest(key, &known)
            .map(|meant| format!("; did you mean `{meant}`?"))
            .unwrap_or_default();
        return Err(at(
            value,
            format!("`{key}` is not a key of a `{kind}` stage{meant}"),
        ));
    }
    let body = |what: &str| -> Result<Vec<Template>, Refused> {
        match get("stages") {
            Some(Item {
                value: Toml::List(list),
                ..
            }) => stages(list, expression)
                .map_err(|(byte, message)| (byte, format!("in its `stages`, {message}"))),
            Some(other) => Err(at(other, "`stages` is a list of stages")),
            None => Err(at(
                item,
                format!("a `{what}` makes its texture with stages = [ … ]"),
            )),
        }
    };
    let built = match kind {
        "pass" => Kind::Fixed(Stage::Pass {
            frag: word(get("pass").unwrap_or(item), "pass")?,
            scale: scale(get("scale"))?,
            format: format(get("format"))?,
            uses: get("uses")
                .map(|uses| words(uses, "uses"))
                .transpose()?
                .unwrap_or_default(),
            input: get("input").map(|input| word(input, "input")).transpose()?,
            per_part: match get("per_part") {
                None => false,
                Some(Item {
                    value: Toml::Bool(yes),
                    ..
                }) => *yes,
                Some(other) => return Err(at(other, "`per_part` is true or false")),
            },
        }),
        "save" => Kind::Fixed(Stage::Save(word(get("save").unwrap_or(item), "save")?)),
        "get" => Kind::Fixed(Stage::Get(word(get("get").unwrap_or(item), "get")?)),
        "use" => {
            let mut params = Vec::new();
            match get("params") {
                None => {}
                Some(Item {
                    value: Toml::Table(given),
                    ..
                }) => {
                    for (name, value) in given {
                        let what = format!("`use`'s `{name}`");
                        let arg = match value.value {
                            Toml::Text(_) => Arg::Expr(expression(value, &what)?),
                            _ => Arg::Literal(literal(value, &what)?),
                        };
                        params.push((name.clone(), arg));
                    }
                }
                Some(other) => {
                    return Err(at(
                        other,
                        "a `use`'s `params` is a table: params = { passes = 2 }",
                    ));
                }
            }
            Kind::Use {
                effect: word(get("use").unwrap_or(item), "use")?,
                params,
            }
        }
        "state" => Kind::State {
            name: word(get("state").unwrap_or(item), "state")?,
            format: format(get("format"))?,
            scale: scale(get("scale"))?,
            depends: match get("depends").map(|depends| (depends, word(depends, "depends"))) {
                Some((_, Ok(word))) if word == "shape" => Depends::Shape,
                Some((_, Ok(word))) if word == "params" => Depends::Params,
                Some((_, Ok(word))) if word == "self" => Depends::SelfCommit,
                Some((_, Ok(word))) if word == "region" => Depends::Region,
                Some((depends, _)) => {
                    return Err(at(
                        depends,
                        "`depends` is \"shape\", \"params\", \"self\" or \"region\"",
                    ));
                }
                None => {
                    return Err(at(
                        item,
                        "a `state` says what it `depends` on: \"shape\", \"params\", \"self\" or \"region\"",
                    ));
                }
            },
            body: body("state")?,
        },
        _ => Kind::Group(body("stages")?),
    };
    let when = match get("when") {
        None => None,
        Some(Item {
            value: Toml::Bool(yes),
            ..
        }) => Some(
            Expr::parse(if *yes { "1" } else { "0" }, &[]).map_err(|message| (None, message))?,
        ),
        Some(when) => Some(expression(when, "`when`")?),
    };
    let times = match get("repeat") {
        None => None,
        Some(Item {
            value: Toml::Int(whole),
            at: start,
        }) => Some(Times::Count(
            Expr::parse(&whole.to_string(), &[]).map_err(|message| (Some(*start), message))?,
        )),
        Some(
            count @ Item {
                value: Toml::Text(_),
                ..
            },
        ) => Some(Times::Count(expression(count, "`repeat`")?)),
        Some(Item {
            value: Toml::Table(over),
            at: start,
        }) => {
            let shape = || {
                (
                    Some(*start),
                    "`repeat` over numbers is { over = [64, 32, …], as = \"<name>\" }".to_owned(),
                )
            };
            let mut numbers = None;
            let mut name = None;
            for (key, value) in over {
                match (key.as_str(), &value.value) {
                    ("over", Toml::List(list)) => {
                        numbers = Some(
                            list.iter()
                                .map(|each| number(each, "`repeat`'s `over`"))
                                .collect::<Result<Vec<f64>, _>>()?,
                        );
                    }
                    ("as", Toml::Text(text)) => name = Some(text.clone()),
                    _ => return Err(shape()),
                }
            }
            let (Some(over), Some(as_name)) = (numbers, name) else {
                return Err(shape());
            };
            Some(Times::Over { over, as_name })
        }
        Some(other) => {
            return Err(at(
                other,
                "`repeat` is a number of times, an expression giving one, or { over = [ … ], as = \"<name>\" }",
            ));
        }
    };
    Ok(Template {
        kind: built,
        when,
        times,
    })
}

/// `scale`: above 0, 1 when not given.
fn scale(item: Option<&Item>) -> Result<f64, Refused> {
    match item {
        None => Ok(1.0),
        Some(item) => number(item, "`scale`").and_then(|scale| {
            if scale > 0.0 {
                Ok(scale)
            } else {
                Err(at(item, "`scale` is a number above 0"))
            }
        }),
    }
}

/// `format`: `rgba8` when not given.
fn format(item: Option<&Item>) -> Result<Format, Refused> {
    match item.map(|item| (item, word(item, "format"))) {
        None => Ok(Format::Rgba8),
        Some((_, Ok(word))) if word == "rgba8" => Ok(Format::Rgba8),
        Some((_, Ok(word))) if word == "rgba16f" => Ok(Format::Rgba16f),
        Some((item, _)) => Err(at(item, "`format` is \"rgba8\" or \"rgba16f\"")),
    }
}

/// A TOML document as a [`Doc`]: a table's keys as its fields, a list as
/// its list part. What a pane style's `effects.toml` is read into, so its
/// rules are read as the window manager's are.
/// `tests::a_styles_rules_read_as_a_doc`.
pub(crate) fn doc(text: &str) -> Result<Doc, (Option<u32>, String)> {
    fn of(item: Item) -> Doc {
        match item.value {
            Toml::Text(text) => Doc::Text(text),
            Toml::Int(int) => Doc::Int(int),
            Toml::Num(number) => Doc::Num(number),
            Toml::Bool(yes) => Doc::Bool(yes),
            Toml::Date => Doc::Other("datetime"),
            Toml::List(list) => Doc::Table(DocTable {
                list: list.into_iter().map(of).collect(),
                ..DocTable::default()
            }),
            Toml::Table(keys) => Doc::Table(DocTable {
                fields: keys
                    .into_iter()
                    .map(|(key, value)| (key, of(value)))
                    .collect(),
                ..DocTable::default()
            }),
        }
    }
    let top =
        parse(text).map_err(|(byte, message)| (byte.map(|byte| line_of(text, byte)), message))?;
    Ok(of(Item {
        at: 0,
        value: Toml::Table(top),
    }))
}

#[cfg(test)]
mod tests {
    use solium_effects::describe::{Kind, Times};
    use solium_effects::spec::{Extent, Given, Value};
    use solium_effects::stage::{Format, Stage};

    use super::{doc, read, read_text};
    use crate::script::Doc;

    /// Every effect folder that ships, Solium's and the default
    /// configuration's, that has an `effect.toml`, reads, and its stages
    /// expand at its defaults.
    #[test]
    fn the_shipped_folders_read() {
        let mut read_any = 0;
        for place in [
            crate::assets::effects(),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("default-config/effects"),
        ] {
            for entry in std::fs::read_dir(&place).expect("the folder") {
                let dir = entry.expect("an entry").path();
                let file = dir.join("effect.toml");
                if !file.is_file() {
                    continue;
                }
                let name = dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned();
                let (spec, described) = read(&name, &file)
                    .unwrap_or_else(|problem| panic!("{}: {}", file.display(), problem.message));
                let (defaults, _) = solium_effects::spec::bind(&spec.params, &[]).expect("binds");
                described
                    .stages(&defaults)
                    .unwrap_or_else(|err| panic!("{name}: {err}"));
                read_any += 1;
            }
        }
        assert!(read_any >= 3, "blur, glass and shade at least: {read_any}");
    }

    #[test]
    fn params_are_read_with_their_kinds_and_ranges() {
        let (spec, _) = read_text(
            r#"
api = 2
frag = "a.frag"
[params]
passes = { default = 3, min = 1, max = 6, int = true }
offset = { default = 3, min = 0 }
tint = [0, 0, 0, 0.3]
on = true
half = 0.5
"#,
        )
        .expect("reads");
        let names: Vec<&str> = spec.params.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["half", "offset", "on", "passes", "tint"], "sorted");
        let param = |name: &str| {
            &spec
                .params
                .iter()
                .find(|(each, _)| each == name)
                .expect(name)
                .1
        };
        assert_eq!(param("passes").default, Value::Int(3));
        assert_eq!(
            (param("passes").min, param("passes").max),
            (Some(1.0), Some(6.0))
        );
        assert_eq!(
            param("offset").default,
            Value::Number(3.0),
            "a whole default is still a float without `int`"
        );
        assert_eq!(param("tint").default, Value::Vec4([0.0, 0.0, 0.0, 0.3]));
        assert_eq!(param("on").default, Value::Bool(true));
        assert_eq!(param("half").default, Value::Number(0.5));
    }

    /// The stages blur and glass write read as templates with their
    /// `repeat`, `when` and `use` params, and `reach` as an expression.
    #[test]
    fn stages_read_with_their_repeats_and_conditions() {
        let (spec, described) = read_text(
            r#"
api = 2
inputs = ["backdrop"]
reach = "ceil(offset * 2 ^ (passes + 1))"
fallback = [{ passes = 2 }, "cheap"]

[params]
passes = { default = 3, min = 1, max = 6, int = true }
offset = { default = 3, min = 0 }

[[stages]]
save = "sharp"

[[stages]]
use = "blur"
when = "passes >= 1"
params = { passes = "passes", offset = 2.5 }

[[stages]]
repeat = "passes"
pass = "down.frag"
scale = 0.5
format = "rgba16f"

[[stages]]
repeat = { over = [64, 32], as = "jump" }
pass = "jump.frag"
uses = ["sharp"]
per_part = true
"#,
        )
        .expect("reads");
        assert_eq!(spec.stages, Given::List);
        assert_eq!(spec.reach, Extent::Function);
        assert_eq!(spec.fallback.len(), 2);
        assert_eq!(described.stages.len(), 4);
        assert!(
            matches!(described.stages[1].kind, Kind::Use { .. })
                && described.stages[1].when.is_some()
        );
        assert!(matches!(described.stages[2].times, Some(Times::Count(_))));
        let (defaults, _) = solium_effects::spec::bind(&spec.params, &[]).expect("binds");
        let stages = described.stages(&defaults).expect("expands");
        assert_eq!(stages.len(), 1 + 1 + 3 + 1);
        assert_eq!(
            stages[2],
            Stage::Pass {
                frag: "down.frag".to_owned(),
                scale: 0.5,
                format: Format::Rgba16f,
                uses: Vec::new(),
                input: None,
                per_part: false,
            }
        );
        assert_eq!(
            stages[1],
            Stage::Use {
                effect: "blur".to_owned(),
                // By name, as a table's keys are read: a `use` binds them by
                // name too.
                params: vec![
                    ("offset".to_owned(), Value::Number(2.5)),
                    ("passes".to_owned(), Value::Number(3.0)),
                ],
            }
        );
        assert!((described.extent(false, 0.0, &defaults).expect("reach") - 48.0).abs() < 1e-12);
    }

    /// A problem names its line, and for a misspelt key or param the one
    /// probably meant.
    #[test]
    fn a_problem_is_at_its_line_and_names_what_was_meant() {
        let dir = crate::effect::host::tests::scratch("described-lines");
        let file = dir.join("effect.toml");
        let cases: [(&str, u32, &str); 8] = [
            ("api = 2\nfrag = \"a.frag\"\nreahc = 2\n", 3, "`reach`"),
            (
                "api = 2\n[params]\npasses = 3\n[[stages]]\npass = \"a.frag\"\nrepeat = \"pases\"\n",
                6,
                "`passes`",
            ),
            (
                "api = 2\n[[stages]]\npass = \"a.frag\"\nsacle = 2\n",
                4,
                "`scale`",
            ),
            (
                "api = 2\n[[stages]]\npass = \"a.frag\"\nsave = \"x\"\n",
                2,
                "one kind",
            ),
            ("api = 1\nfrag = \"a.frag\"\n", 1, "api 2"),
            (
                "api = 2\nfrag = \"a.frag\"\n[params]\nmode = \"fast\"\n",
                4,
                "word",
            ),
            (
                "api = 2\nfrag = \"a.frag\"\n[params]\ntint = [0, 0, 0, 1]\n[[stages]]\nget = \"x\"\nwhen = \"tint > 0\"\n",
                7,
                "four numbers",
            ),
            ("api = 2\nfrag = \"a.frag\"\nnot toml\n", 3, "not TOML"),
        ];
        for (text, line, said) in cases {
            std::fs::write(&file, text).expect("writing");
            let problem = read("lines", &file).expect_err(text);
            assert_eq!(problem.line, Some(line), "{text}: {}", problem.message);
            assert!(
                problem.message.contains(said),
                "{text}: {}",
                problem.message
            );
        }
    }

    #[test]
    fn a_styles_rules_read_as_a_doc() {
        let read = doc(r#"
[[rules]]
match = { fullscreen = false }
part = "pane"
slot = "behind"
effect = "glass"
fade = { duration = 240, easing = "outCubic" }
"#)
        .expect("reads");
        let Doc::Table(top) = read else {
            panic!("a table")
        };
        let Some(Doc::Table(rules)) = top.fields.get("rules") else {
            panic!("rules")
        };
        let Some(Doc::Table(rule)) = rules.list.first() else {
            panic!("a rule")
        };
        assert_eq!(
            rule.fields.get("effect"),
            Some(&Doc::Text("glass".to_owned()))
        );
        let Some(Doc::Table(fade)) = rule.fields.get("fade") else {
            panic!("a fade")
        };
        assert_eq!(fade.fields.get("duration"), Some(&Doc::Int(240)));
        assert_eq!(doc("rules = [").expect_err("broken").0, Some(1));
    }
}
