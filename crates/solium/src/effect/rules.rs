//! Effect rules (\[16\] §1): which effect goes in which slot of which part,
//! for which windows or surfaces. Pure: nothing here draws or binds.
//!
//! A rule is read whole from a [`Tree`], its list part and its named keys
//! together, and a broken one is refused with its number, its key and, for a
//! part or a slot, what was probably meant (Ruling 13).
//! `tests::an_unknown_part_is_refused_with_its_rule_number_and_a_suggestion`.

use solium_effects::spec::Value;

use crate::effect::tree::Tree;

/// A string match: exact, `*`, or a prefix ending in `*`; no Lua patterns,
/// which could not be evaluated per frame in Rust (Ruling 13).
/// `tests::match_star_matches_everything_and_a_trailing_star_is_a_prefix`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Glob {
    Any,
    Exact(String),
    Prefix(String),
}

/// Where an effect is drawn (Ruling 15). `part = "output"` is no variant: it
/// is refused until X4.2 adds it.
/// `tests::what_waits_for_a_later_item_is_refused_by_name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    Pane,
    Client,
    Popup,
    Layer(String),
    Region(String),
    Surface(String),
    LayerShell(Glob),
}

/// What a part belongs to, which decides the match keys a rule for it may
/// use. `tests::a_match_key_of_another_family_is_refused`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    Window,
    Surface,
    LayerShell,
}

/// Where around a part an effect sits: below it, above it, or in its place.
/// `tests::a_rule_reads_its_part_slot_match_and_chain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Slot {
    Behind,
    Front,
    Replace,
}

/// What a rule's `match` read; a key not given matches anything, so
/// `match = "*"` sets none. `tests::every_match_key_is_read_for_its_family`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Match {
    pub(crate) app_id: Option<Glob>,
    pub(crate) title: Option<Glob>,
    pub(crate) focused: Option<bool>,
    pub(crate) fullscreen: Option<bool>,
    pub(crate) monitor: Option<Glob>,
    pub(crate) style: Option<Glob>,
    pub(crate) surface: Option<Glob>,
    pub(crate) layer_shell: Option<Glob>,
}

/// What a chain's first input is rebound to: `Own` is `source = "self"`.
/// `tests::the_rest_of_a_rule_is_read_as_written`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    Xray,
    Live,
    Auto,
    Own,
}

/// How the result is cut: the part's shape, or the alpha of the self capture
/// `source = "self"` makes; an alpha without one waits for X4.3.
/// `tests::the_rest_of_a_rule_is_read_as_written`,
/// `tests::what_waits_for_a_later_item_is_refused_by_name`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum MaskKind {
    #[default]
    Shape,
    Alpha,
}

/// A link's `reach` and `bleed`, which override its file's for this binding
/// (Ruling 5). `tests::a_links_reach_and_bleed_override_the_files`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Extents {
    pub(crate) reach: Option<f64>,
    pub(crate) bleed: Option<f64>,
}

/// One effect of a chain, with the params its rule gives it.
/// `tests::a_mixed_table_keeps_its_params_beside_the_name`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Link {
    pub(crate) effect: String,
    pub(crate) params: Vec<(String, Value)>,
    pub(crate) extents: Extents,
}

/// What a slot holds: nothing (`effect = false`), or a chain in order.
/// `tests::effect_false_is_an_empty_slot`, `tests::a_list_of_tables_is_a_chain_in_order`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fill {
    Off,
    Chain(Vec<Link>),
}

/// One rule, read. `tests::a_rule_reads_its_part_slot_match_and_chain`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rule {
    pub(crate) matches: Match,
    pub(crate) part: Part,
    pub(crate) slot: Slot,
    pub(crate) fill: Fill,
    pub(crate) source: Option<Source>,
    pub(crate) mask: MaskKind,
}

/// Where a list of rules came from, in the order a later one wins for one
/// part and slot (Ruling 13); material and expansion are empty in FX2.
/// `tests::the_origins_are_ordered_style_material_expansion_user`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Origin {
    Style,
    Material,
    Expansion,
    User,
}

/// A rule refused: its number counting from 1, as Lua does, and its key.
/// `tests::a_broken_rule_is_refused_at_its_key`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RuleError {
    pub(crate) rule: usize,
    pub(crate) key: &'static str,
    pub(crate) message: String,
}

/// The parts a word names whole, and the kinds of part a name follows.
/// `tests::a_broken_rule_is_refused_at_its_key`.
const PARTS: [&str; 4] = ["pane", "client", "popup", "region:titlebar"];
const KINDS: [&str; 4] = ["layer", "region", "surface", "layer_shell"];
const PART_LIST: &str =
    "pane, client, popup, layer:<name>, region:titlebar, surface:<name> or layer_shell:<namespace>";
const SLOTS: [&str; 3] = ["behind", "front", "replace"];
const KEYS: [&str; 6] = ["match", "part", "slot", "effect", "source", "mask"];
const WINDOW_KEYS: [&str; 6] = [
    "app_id",
    "title",
    "focused",
    "fullscreen",
    "monitor",
    "style",
];
const MATCH_KEYS: [&str; 8] = [
    "app_id",
    "title",
    "focused",
    "fullscreen",
    "monitor",
    "style",
    "surface",
    "layer_shell",
];
const SOURCES: [&str; 4] = ["xray", "live", "auto", "self"];
const MASKS: [&str; 2] = ["shape", "alpha"];
const NO_EFFECT: &str = "`effect` names no effect: give its name, a link such as { \"blur\", passes = 3 }, a chain of links, or false to empty the slot";
const NOT_A_LINK: &str = "a chain is a list of links, each a name or { \"name\", params }";

/// What `nearest` found among `among`, as the end of a message.
/// `tests::a_broken_rule_is_refused_at_its_key`.
fn meant(word: &str, among: &[&str]) -> String {
    solium_effects::spec::nearest(word, among)
        .map(|meant| format!("; did you mean `{meant}`?"))
        .unwrap_or_default()
}

/// `*` ends a prefix and appears nowhere else (Ruling 13).
/// `tests::a_broken_rule_is_refused_at_its_key`.
fn glob_word(text: &str) -> Result<Glob, String> {
    if text.trim_end_matches('*').contains('*') || text.ends_with("**") {
        return Err(format!(
            "`{text}`: `*` only ends a prefix, such as `org.gnome.*`; there are no other patterns"
        ));
    }
    Ok(Glob::parse(text))
}

impl Glob {
    /// Exact, `*`, or a prefix ending in `*` (Ruling 13).
    /// `tests::match_star_matches_everything_and_a_trailing_star_is_a_prefix`.
    pub(crate) fn parse(text: &str) -> Self {
        match text.strip_suffix('*') {
            Some("") => Self::Any,
            Some(prefix) => Self::Prefix(prefix.to_owned()),
            None => Self::Exact(text.to_owned()),
        }
    }

    /// `tests::match_star_matches_everything_and_a_trailing_star_is_a_prefix`.
    pub(crate) fn matches(&self, value: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(exact) => value == exact,
            Self::Prefix(prefix) => value.starts_with(prefix.as_str()),
        }
    }
}

impl Part {
    /// `tests::what_waits_for_a_later_item_is_refused_by_name`,
    /// `tests::an_unknown_part_is_refused_with_its_rule_number_and_a_suggestion`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text {
            "pane" => return Ok(Self::Pane),
            "client" => return Ok(Self::Client),
            "popup" => return Ok(Self::Popup),
            "output" => return Err("`part = \"output\"` arrives with X4.2 (FX6)".to_owned()),
            _ => {}
        }
        let Some((kind, name)) = text
            .split_once(':')
            .filter(|(kind, _)| KINDS.contains(kind))
        else {
            let meant = solium_effects::spec::nearest(text, &PARTS)
                .map(str::to_owned)
                .or_else(|| {
                    let (kind, name) = text.split_once(':')?;
                    solium_effects::spec::nearest(kind, &KINDS).map(|kind| format!("{kind}:{name}"))
                })
                .map(|meant| format!("; did you mean `{meant}`?"))
                .unwrap_or_default();
            return Err(format!("`{text}` is not a part: {PART_LIST}{meant}"));
        };
        if name.is_empty() {
            return Err(format!("`{text}` needs a name after the colon"));
        }
        Ok(match kind {
            "layer" => Self::Layer(name.to_owned()),
            "region" if name == "titlebar" => Self::Region(name.to_owned()),
            "region" => {
                return Err(format!(
                    "`region:{name}`: regions other than the titlebar arrive with P15's Solium.region"
                ));
            }
            "surface" => Self::Surface(name.to_owned()),
            _ => Self::LayerShell(glob_word(name)?),
        })
    }

    /// `tests::every_match_key_is_read_for_its_family`.
    pub(crate) fn family(&self) -> Family {
        match self {
            Self::Pane | Self::Client | Self::Popup | Self::Layer(_) | Self::Region(_) => {
                Family::Window
            }
            Self::Surface(_) => Family::Surface,
            Self::LayerShell(_) => Family::LayerShell,
        }
    }
}

/// Parse a list of rules. Every error is collected, each with its rule number
/// (counting from 1, as Lua does) and its key.
/// `tests::a_broken_rule_is_refused_at_its_key`.
pub(crate) fn parse(rules: &Tree) -> Result<Vec<Rule>, Vec<RuleError>> {
    let mut read = Vec::new();
    let mut errors = Vec::new();
    for (index, each) in rules.list().iter().enumerate() {
        match rule(each) {
            Ok(rule) => read.push(rule),
            Err((key, message)) => errors.push(RuleError {
                rule: index + 1,
                key,
                message,
            }),
        }
    }
    if errors.is_empty() {
        Ok(read)
    } else {
        Err(errors)
    }
}

type Refused = (&'static str, String);

/// One rule, or the key it is refused at and why.
/// `tests::a_broken_rule_is_refused_at_its_key`.
fn rule(each: &Tree) -> Result<Rule, Refused> {
    let Tree::Table { list, fields } = each else {
        return Err((
            "rule",
            "a rule is a table: { match = …, part = …, slot = …, effect = … }".to_owned(),
        ));
    };
    if !list.is_empty() {
        return Err((
            "rule",
            "a rule's keys are named: { match = …, part = …, slot = …, effect = … }".to_owned(),
        ));
    }
    for key in fields.keys() {
        if key == "keep" {
            return Err(("keep", "`keep` arrives with X2.7 (FX3)".to_owned()));
        }
        if !KEYS.contains(&key.as_str()) {
            return Err(match solium_effects::spec::nearest(key, &KEYS) {
                Some(meant) => (
                    meant,
                    format!("`{key}` is not a rule key; did you mean `{meant}`?"),
                ),
                None => (
                    "rule",
                    format!("`{key}` is not a rule key: match, part, slot, effect, source or mask"),
                ),
            });
        }
    }
    let needs = |key: &'static str, what: &str| (key, format!("a rule needs `{key}`: {what}"));
    let part = match each.field("part") {
        None => return Err(needs("part", PART_LIST)),
        Some(Tree::Text(text)) => Part::parse(text).map_err(|message| ("part", message))?,
        Some(_) => return Err(("part", format!("`part` is a word: {PART_LIST}"))),
    };
    let slot = match each.field("slot") {
        None => return Err(needs("slot", "behind, front or replace")),
        Some(Tree::Text(text)) => match text.as_str() {
            "behind" => Slot::Behind,
            "front" => Slot::Front,
            "replace" => Slot::Replace,
            _ => {
                return Err((
                    "slot",
                    format!(
                        "`{text}` is not a slot: behind, front or replace{}",
                        meant(text, &SLOTS)
                    ),
                ));
            }
        },
        Some(_) => return Err(("slot", "`slot` is behind, front or replace".to_owned())),
    };
    let matches = match each.field("match") {
        None => {
            return Err(needs(
                "match",
                "\"*\" or a table such as { app_id = \"mpv\" }",
            ));
        }
        Some(matched) => read_match(matched, &part).map_err(|message| ("match", message))?,
    };
    let (fill, linked) = match each.field("effect") {
        None => {
            return Err(needs(
                "effect",
                "an effect's name, a link, a chain, or false to empty the slot",
            ));
        }
        Some(effect) => read_fill(effect)?,
    };
    let source = match (each.field("source").map(read_source).transpose()?, linked) {
        (Some(_), Some(_)) => {
            return Err((
                "source",
                "`source` is given twice, on the rule and in its first link: give it once"
                    .to_owned(),
            ));
        }
        (given, linked) => given.or(linked),
    };
    let mask = match each.field("mask") {
        None => MaskKind::Shape,
        Some(Tree::Text(text)) if text == "shape" => MaskKind::Shape,
        Some(Tree::Text(text)) if text == "alpha" => MaskKind::Alpha,
        Some(Tree::Text(text)) => {
            return Err((
                "mask",
                format!(
                    "`{text}` is not a mask: shape or alpha{}",
                    meant(text, &MASKS)
                ),
            ));
        }
        Some(_) => return Err(("mask", "`mask` is shape or alpha".to_owned())),
    };
    // The alpha is the self capture's (Ruling 15), and only `source = "self"`
    // says at parse that one exists: what an effect reads is not known until
    // it is bound (Ruling 14), so an alpha without it waits for X4.3
    // (Ruling 13). `tests::what_waits_for_a_later_item_is_refused_by_name`.
    if mask == MaskKind::Alpha && source != Some(Source::Own) {
        return Err((
            "mask",
            "`mask = \"alpha\"` is the self capture's alpha: give `source = \"self\"`; an alpha mask without a self capture arrives with X4.3"
                .to_owned(),
        ));
    }
    Ok(Rule {
        matches,
        part,
        slot,
        fill,
        source,
        mask,
    })
}

/// `match`: `"*"`, or a table of the keys the part's family has.
/// `tests::every_match_key_is_read_for_its_family`.
fn read_match(matched: &Tree, part: &Part) -> Result<Match, String> {
    let fields = match matched {
        Tree::Text(text) if text == "*" => return Ok(Match::default()),
        Tree::Table { list, fields } if list.is_empty() => fields,
        Tree::Table { .. } => {
            return Err("`match` takes named keys, such as { app_id = \"mpv\" }".to_owned());
        }
        _ => return Err("`match` is \"*\" or a table such as { app_id = \"mpv\" }".to_owned()),
    };
    let (family_keys, whose): (&[&str], &str) = match part.family() {
        Family::Window => (&WINDOW_KEYS, "a window's"),
        Family::Surface => (&["surface"], "a scripted surface's"),
        Family::LayerShell => (&["layer_shell"], "a layer surface's"),
    };
    let mut read = Match::default();
    for (key, value) in fields {
        if !MATCH_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "`{key}` is not a match key for {whose} part: {}{}",
                family_keys.join(", "),
                meant(key, family_keys)
            ));
        }
        if !family_keys.contains(&key.as_str()) {
            return Err(format!(
                "`{key}` does not match {whose} part: match it with {}",
                family_keys.join(", ")
            ));
        }
        if let "focused" | "fullscreen" = key.as_str() {
            let Tree::Bool(yes) = value else {
                return Err(format!("`{key}` is true or false"));
            };
            if key == "focused" {
                read.focused = Some(*yes);
            } else {
                read.fullscreen = Some(*yes);
            }
            continue;
        }
        let Tree::Text(text) = value else {
            return Err(format!(
                "`{key}` is a word: exact, \"*\", or a prefix ending in `*`"
            ));
        };
        if key == "surface" && text.contains('/') {
            return Err(format!(
                "`{text}`: matching a plane (`shell/dock`) arrives with P8"
            ));
        }
        let glob = Some(glob_word(text)?);
        match key.as_str() {
            "app_id" => read.app_id = glob,
            "title" => read.title = glob,
            "monitor" => read.monitor = glob,
            "style" => read.style = glob,
            "surface" => read.surface = glob,
            _ => read.layer_shell = glob,
        }
    }
    Ok(read)
}

/// `effect`: `false`, a name, a link, or a chain of links; and the `source`
/// its first link gave. `tests::a_list_of_tables_is_a_chain_in_order`.
fn read_fill(effect: &Tree) -> Result<(Fill, Option<Source>), Refused> {
    let refused = |message: &str| ("effect", message.to_owned());
    match effect {
        Tree::Bool(false) => Ok((Fill::Off, None)),
        Tree::Text(_) => {
            let (link, _) = read_link(effect, true)?;
            Ok((Fill::Chain(vec![link]), None))
        }
        Tree::Table { list, fields } => match list.first() {
            Some(Tree::Text(_)) => {
                let (link, source) = read_link(effect, true)?;
                Ok((Fill::Chain(vec![link]), source))
            }
            Some(Tree::Table { .. }) => {
                if !fields.is_empty() {
                    return Err(refused(
                        "a chain's params go in its links: { { \"blur\", passes = 3 }, { \"tint\" } }",
                    ));
                }
                let mut links = Vec::new();
                let mut source = None;
                for (index, each) in list.iter().enumerate() {
                    let (link, given) = read_link(each, index == 0)?;
                    links.push(link);
                    source = source.or(given);
                }
                Ok((Fill::Chain(links), source))
            }
            None if !fields.is_empty() => Err(refused(
                "a link starts with its effect's name: { \"blur\", passes = 3 }",
            )),
            _ => Err(refused(NO_EFFECT)),
        },
        Tree::Bool(true) | Tree::Number(_) | Tree::Int(_) => Err(refused(NO_EFFECT)),
    }
}

/// One link: a name, or a table whose `[1]` is the name and whose named keys
/// are its params, its `reach` and `bleed`, and (in a first link) `source`.
/// `tests::a_mixed_table_keeps_its_params_beside_the_name`,
/// `tests::a_links_reach_and_bleed_override_the_files`.
fn read_link(link: &Tree, first: bool) -> Result<(Link, Option<Source>), Refused> {
    let refused = |message: String| ("effect", message);
    let (name, fields) = match link {
        Tree::Text(name) => (name, None),
        Tree::Table { list, fields } => match list.as_slice() {
            [Tree::Text(name)] => (name, Some(fields)),
            [Tree::Text(_), ..] => {
                return Err(refused(
                    "a link is one effect's name and its params; a chain is a list of links: { { \"blur\" }, { \"tint\" } }"
                        .to_owned(),
                ));
            }
            _ => return Err(refused(NOT_A_LINK.to_owned())),
        },
        _ => return Err(refused(NOT_A_LINK.to_owned())),
    };
    let mut read = Link {
        effect: name.clone(),
        params: Vec::new(),
        extents: Extents::default(),
    };
    let mut source = None;
    for (key, value) in fields.into_iter().flatten() {
        match key.as_str() {
            "source" if !first => {
                return Err(refused(
                    "`source` goes on the rule or in the first link: it rebinds the chain's first input"
                        .to_owned(),
                ));
            }
            "source" => source = Some(read_source(value)?),
            "reach" | "bleed" => {
                let pixels = match value {
                    Tree::Number(number) => Some(*number),
                    #[expect(clippy::cast_precision_loss, reason = "a number of pixels")]
                    Tree::Int(int) => Some(*int as f64),
                    _ => None,
                };
                let Some(pixels) = pixels.filter(|pixels| *pixels >= 0.0) else {
                    return Err(refused(format!("`{key}` is a number of pixels, 0 or more")));
                };
                if key == "reach" {
                    read.extents.reach = Some(pixels);
                } else {
                    read.extents.bleed = Some(pixels);
                }
            }
            _ => match value.value() {
                Some(value) => read.params.push((key.clone(), value)),
                None => {
                    return Err(refused(format!(
                        "`{key}` is not a param value: a number, a boolean, a word or four numbers"
                    )));
                }
            },
        }
    }
    Ok((read, source))
}

/// `source`: what the chain's first input is rebound to.
/// `tests::the_rest_of_a_rule_is_read_as_written`.
fn read_source(source: &Tree) -> Result<Source, Refused> {
    match source {
        Tree::Text(text) => match text.as_str() {
            "xray" => Ok(Source::Xray),
            "live" => Ok(Source::Live),
            "auto" => Ok(Source::Auto),
            "self" => Ok(Source::Own),
            _ => Err((
                "source",
                format!(
                    "`{text}` is not a source: xray, live, auto or self{}",
                    meant(text, &SOURCES)
                ),
            )),
        },
        _ => Err(("source", "`source` is xray, live, auto or self".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use solium_effects::spec::Value;

    use super::{Fill, Glob, Link, MaskKind, Part, Slot, Source, parse};
    use crate::effect::tree::Tree;

    /// Rules from Lua source, read the way `sol.effects` reads them.
    fn rules(lua: &str) -> Tree {
        let state = mlua::Lua::new();
        let value: mlua::Value = state.load(lua).eval().expect("the test's Lua");
        Tree::from_lua(&value).expect("readable").expect("a value")
    }

    #[test]
    fn a_rule_reads_its_part_slot_match_and_chain() {
        let read = parse(&rules(
            r#"{ { match = { app_id = "mpv" }, part = "client", slot = "replace", effect = { "blur", source = "self", passes = 3 } } }"#,
        ))
        .expect("parses");
        assert_eq!(read.len(), 1);
        let rule = &read[0];
        assert_eq!(
            (rule.part.clone(), rule.slot),
            (Part::Client, Slot::Replace)
        );
        assert_eq!(rule.matches.app_id, Some(Glob::Exact("mpv".to_owned())));
        assert_eq!(rule.source, Some(Source::Own));
        assert_eq!(
            rule.fill,
            Fill::Chain(vec![Link {
                effect: "blur".to_owned(),
                params: vec![("passes".to_owned(), Value::Int(3))],
                extents: super::Extents::default()
            }])
        );
        assert_eq!(rule.mask, MaskKind::Shape);
    }

    /// **`reach` and `bleed` in a link override the file's** (Ruling 5), so
    /// \[16\] §1's `{ "burn", bleed = 48 }` reads as written; they are not
    /// params, and a non-number is refused.
    #[test]
    fn a_links_reach_and_bleed_override_the_files() {
        let read = parse(&rules(
            r#"{ { match = "*", part = "client", slot = "behind", effect = { "glow", reach = 12, bleed = 4, radius = 2 } } }"#,
        ))
        .expect("parses");
        let Fill::Chain(links) = &read[0].fill else {
            panic!("a chain")
        };
        assert_eq!(
            links[0].extents,
            super::Extents {
                reach: Some(12.0),
                bleed: Some(4.0)
            }
        );
        assert_eq!(links[0].params, vec![("radius".to_owned(), Value::Int(2))]);
        let errors = parse(&rules(
            r#"{ { match = "*", part = "client", slot = "behind", effect = { "glow", bleed = "lots" } } }"#,
        ))
        .expect_err("refused");
        assert_eq!(errors[0].key, "effect");
    }

    /// **A mixed table keeps its params beside the name**: the shape
    /// `Json::from_lua` loses.
    #[test]
    fn a_mixed_table_keeps_its_params_beside_the_name() {
        let read = parse(&rules(
            r#"{ { match = "*", part = "pane", slot = "behind", effect = { "glow", radius = 12, tint = { 1, 0, 0, 1 } } } }"#,
        ))
        .expect("parses");
        let Fill::Chain(links) = &read[0].fill else {
            panic!("a chain")
        };
        assert_eq!(
            links[0].params,
            vec![
                ("radius".to_owned(), Value::Int(12)),
                ("tint".to_owned(), Value::Vec4([1.0, 0.0, 0.0, 1.0]))
            ]
        );
    }

    #[test]
    fn a_list_of_tables_is_a_chain_in_order() {
        let read = parse(&rules(
            r#"{ { match = "*", part = "client", slot = "behind", effect = { { "blur", source = "self" }, { "tint", amount = 0.1 } } } }"#,
        ))
        .expect("parses");
        let Fill::Chain(links) = &read[0].fill else {
            panic!("a chain")
        };
        assert_eq!(
            links
                .iter()
                .map(|link| link.effect.as_str())
                .collect::<Vec<_>>(),
            ["blur", "tint"]
        );
        assert_eq!(
            read[0].source,
            Some(Source::Own),
            "source in the first link is the rule's"
        );
    }

    #[test]
    fn effect_false_is_an_empty_slot() {
        let read = parse(&rules(
            r#"{ { match = "*", part = "region:titlebar", slot = "behind", effect = false } }"#,
        ))
        .expect("parses");
        assert_eq!(read[0].fill, Fill::Off);
        assert_eq!(read[0].part, Part::Region("titlebar".to_owned()));
    }

    #[test]
    fn an_unknown_part_is_refused_with_its_rule_number_and_a_suggestion() {
        let errors = parse(&rules(
            r#"{ { match = "*", part = "pane", slot = "behind", effect = false }, { match = "*", part = "regoin:titlebar", slot = "behind", effect = false } }"#,
        ))
        .expect_err("refused");
        assert_eq!((errors[0].rule, errors[0].key), (2, "part"));
        assert!(
            errors[0].message.contains("region:titlebar"),
            "{:?}",
            errors[0]
        );
    }

    #[test]
    fn a_match_key_of_another_family_is_refused() {
        let errors = parse(&rules(
            r#"{ { match = { surface = "bar" }, part = "client", slot = "front", effect = false } }"#,
        ))
        .expect_err("refused");
        assert_eq!(errors[0].key, "match");
    }

    #[test]
    fn source_inside_a_later_link_is_refused() {
        let errors = parse(&rules(
            r#"{ { match = "*", part = "client", slot = "behind", effect = { { "blur" }, { "tint", source = "self" } } } }"#,
        ))
        .expect_err("refused");
        assert_eq!(errors[0].key, "effect");
    }

    /// What waits for a later item is refused, naming it.
    #[test]
    fn what_waits_for_a_later_item_is_refused_by_name() {
        for (lua, names) in [
            (
                r#"{ { match = "*", part = "output", slot = "front", effect = false } }"#,
                "X4.2",
            ),
            (
                r#"{ { match = "*", part = "region:bar", slot = "behind", effect = false } }"#,
                "P15",
            ),
            (
                r#"{ { match = { surface = "shell/dock" }, part = "surface:shell", slot = "behind", effect = false } }"#,
                "P8",
            ),
            (
                r#"{ { match = "*", part = "client", slot = "behind", effect = "blur", keep = true } }"#,
                "X2.7",
            ),
            (
                r#"{ { match = "*", part = "client", slot = "behind", effect = "blur", source = "live", mask = "alpha" } }"#,
                "X4.3",
            ),
            (
                r#"{ { match = "*", part = "client", slot = "behind", effect = "glow", mask = "alpha" } }"#,
                "X4.3",
            ),
        ] {
            let errors = parse(&rules(lua)).expect_err(lua);
            assert!(errors[0].message.contains(names), "{lua}: {:?}", errors[0]);
        }
    }

    #[test]
    fn match_star_matches_everything_and_a_trailing_star_is_a_prefix() {
        assert!(Glob::parse("*").matches("anything"));
        assert!(Glob::parse("org.gnome.*").matches("org.gnome.Nautilus"));
        assert!(!Glob::parse("org.gnome.*").matches("org.kde.dolphin"));
        assert!(Glob::parse("mpv").matches("mpv") && !Glob::parse("mpv").matches("mpv2"));
        assert_eq!(
            Part::parse("layer_shell:*").expect("parses"),
            Part::LayerShell(Glob::Any)
        );
    }

    /// Every window key is read, and a scripted surface's and a layer
    /// surface's own; an empty `match` is `"*"`.
    #[test]
    fn every_match_key_is_read_for_its_family() {
        let read = parse(&rules(
            r#"{ { match = { app_id = "org.gnome.*", title = "a film", focused = true, fullscreen = false, monitor = "DP-*", style = "top" }, part = "pane", slot = "front", effect = false },
                 { match = { surface = "bar" }, part = "surface:bar", slot = "behind", effect = false },
                 { match = { layer_shell = "waybar*" }, part = "layer_shell:waybar", slot = "behind", effect = false },
                 { match = {}, part = "layer:shadow", slot = "behind", effect = false } }"#,
        ))
        .expect("parses");
        let window = &read[0].matches;
        assert_eq!(
            (
                window.app_id.clone(),
                window.title.clone(),
                window.monitor.clone(),
                window.style.clone()
            ),
            (
                Some(Glob::Prefix("org.gnome.".to_owned())),
                Some(Glob::Exact("a film".to_owned())),
                Some(Glob::Prefix("DP-".to_owned())),
                Some(Glob::Exact("top".to_owned()))
            )
        );
        assert_eq!(
            (window.focused, window.fullscreen),
            (Some(true), Some(false))
        );
        assert_eq!(read[1].matches.surface, Some(Glob::Exact("bar".to_owned())));
        assert_eq!(read[1].part.family(), super::Family::Surface);
        assert_eq!(
            read[2].matches.layer_shell,
            Some(Glob::Prefix("waybar".to_owned()))
        );
        assert_eq!(
            read[2].part,
            Part::LayerShell(Glob::Exact("waybar".to_owned()))
        );
        assert_eq!(read[2].part.family(), super::Family::LayerShell);
        assert_eq!(read[3].matches, super::Match::default());
        assert_eq!(read[3].part, Part::Layer("shadow".to_owned()));
        assert_eq!(read[3].part.family(), super::Family::Window);
    }

    /// `source` and `mask` on the rule, a bare name in a chain, and a
    /// `reach` of 0 with no `bleed`; an alpha mask over a self capture
    /// named on the rule or in the first link.
    #[test]
    fn the_rest_of_a_rule_is_read_as_written() {
        let read = parse(&rules(
            r#"{ { match = "*", part = "popup", slot = "front", effect = { { "glow", reach = 0 }, "tint" }, source = "self", mask = "alpha" },
                 { match = "*", part = "client", slot = "behind", effect = "glow", source = "xray" },
                 { match = "*", part = "client", slot = "front", effect = "glow", source = "auto", mask = "shape" },
                 { match = "*", part = "client", slot = "replace", effect = "glow", source = "live" },
                 { match = "*", part = "client", slot = "front", effect = { "blur", source = "self" }, mask = "alpha" } }"#,
        ))
        .expect("parses");
        assert_eq!(
            (read[0].source, read[0].mask),
            (Some(Source::Own), MaskKind::Alpha)
        );
        let Fill::Chain(links) = &read[0].fill else {
            panic!("a chain")
        };
        assert_eq!(
            (links[0].extents.reach, links[0].extents.bleed),
            (Some(0.0), None)
        );
        assert_eq!(
            links[1],
            Link {
                effect: "tint".to_owned(),
                params: Vec::new(),
                extents: super::Extents::default()
            }
        );
        assert_eq!(
            (read[1].source, read[1].mask),
            (Some(Source::Xray), MaskKind::Shape)
        );
        assert_eq!(
            (read[2].source, read[2].mask),
            (Some(Source::Auto), MaskKind::Shape)
        );
        assert_eq!(
            (read[3].source, read[3].mask),
            (Some(Source::Live), MaskKind::Shape)
        );
        assert_eq!(
            (read[4].source, read[4].mask),
            (Some(Source::Own), MaskKind::Alpha)
        );
    }

    /// Each way a rule can be broken is refused at its key, never read as a
    /// default; a rule after a broken one is still read and refused alone.
    #[test]
    fn a_broken_rule_is_refused_at_its_key() {
        for (lua, key, says) in [
            (r#""client""#, "rule", "a table"),
            (r#"{ "*", "client", "front", false }"#, "rule", "named"),
            (
                r#"{ part = "client", slot = "front", effect = false }"#,
                "match",
                "needs `match`",
            ),
            (
                r#"{ match = "*", slot = "front", effect = false }"#,
                "part",
                "needs `part`",
            ),
            (
                r#"{ match = "*", part = "client", effect = false }"#,
                "slot",
                "needs `slot`",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front" }"#,
                "effect",
                "needs `effect`",
            ),
            (
                r#"{ match = "*", part = "client", slto = "front", effect = false }"#,
                "slot",
                "did you mean `slot`",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = false, wobble = 1 }"#,
                "rule",
                "`wobble`",
            ),
            (
                r#"{ match = "*", part = "clinet", slot = "front", effect = false }"#,
                "part",
                "did you mean `client`",
            ),
            (
                r#"{ match = "*", part = "layr:shadow", slot = "front", effect = false }"#,
                "part",
                "did you mean `layer:shadow`",
            ),
            (
                r#"{ match = "*", part = "layer:", slot = "front", effect = false }"#,
                "part",
                "a name",
            ),
            (
                r#"{ match = "*", part = "layer_shell:", slot = "front", effect = false }"#,
                "part",
                "a name",
            ),
            (
                r#"{ match = "*", part = "layer_shell:*bar", slot = "front", effect = false }"#,
                "part",
                "prefix",
            ),
            (
                r#"{ match = "*", part = "client", slot = "fornt", effect = false }"#,
                "slot",
                "did you mean `front`",
            ),
            (
                r#"{ match = "mpv", part = "client", slot = "front", effect = false }"#,
                "match",
                r#""*""#,
            ),
            (
                r#"{ match = { "mpv" }, part = "client", slot = "front", effect = false }"#,
                "match",
                "named",
            ),
            (
                r#"{ match = { appid = "mpv" }, part = "client", slot = "front", effect = false }"#,
                "match",
                "did you mean `app_id`",
            ),
            (
                r#"{ match = { app_id = "*mpv" }, part = "client", slot = "front", effect = false }"#,
                "match",
                "prefix",
            ),
            (
                r#"{ match = { app_id = 3 }, part = "client", slot = "front", effect = false }"#,
                "match",
                "a word",
            ),
            (
                r#"{ match = { focused = "yes" }, part = "client", slot = "front", effect = false }"#,
                "match",
                "true or false",
            ),
            (
                r#"{ match = { app_id = "mpv" }, part = "surface:bar", slot = "front", effect = false }"#,
                "match",
                "surface",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = true }"#,
                "effect",
                "names no effect",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = {} }"#,
                "effect",
                "names no effect",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = 3 }"#,
                "effect",
                "names no effect",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = { passes = 3 } }"#,
                "effect",
                "starts with",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = { "blur", "tint" } }"#,
                "effect",
                "a chain is a list of links",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = { { "blur" }, { "tint" }, amount = 0.1 } }"#,
                "effect",
                "in its links",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = { { "blur" }, { { "tint" } } } }"#,
                "effect",
                "a chain is a list of links",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = { "blur", tint = { 1, 0 } } }"#,
                "effect",
                "`tint`",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = { "blur", reach = -1 } }"#,
                "effect",
                "0 or more",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = { "blur", source = "self" }, source = "self" }"#,
                "source",
                "twice",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = "blur", source = "slef" }"#,
                "source",
                "did you mean `self`",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = { "blur", source = 1 } }"#,
                "source",
                "xray, live, auto or self",
            ),
            (
                r#"{ match = "*", part = "client", slot = "front", effect = "blur", mask = "alhpa" }"#,
                "mask",
                "did you mean `alpha`",
            ),
        ] {
            let list = format!(
                r#"{{ {{ match = "*", part = "pane", slot = "behind", effect = false }}, {lua}, {{ match = "*", part = "client", slot = "behind", effect = "nothing-wrong" }} }}"#
            );
            let errors = parse(&rules(&list)).expect_err(lua);
            assert_eq!(errors.len(), 1, "{lua}: {errors:?}");
            assert_eq!(
                (errors[0].rule, errors[0].key),
                (2, key),
                "{lua}: {:?}",
                errors[0]
            );
            assert!(errors[0].message.contains(says), "{lua}: {:?}", errors[0]);
        }
    }

    #[test]
    fn the_origins_are_ordered_style_material_expansion_user() {
        use super::Origin;
        let mut origins = [
            Origin::User,
            Origin::Expansion,
            Origin::Style,
            Origin::Material,
        ];
        origins.sort();
        assert_eq!(
            origins,
            [
                Origin::Style,
                Origin::Material,
                Origin::Expansion,
                Origin::User
            ]
        );
    }
}
