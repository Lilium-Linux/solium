//! Effect rules (\[16\] §1): which effect goes in which slot of which part,
//! for which windows or surfaces. Pure: nothing here draws or binds.
//!
//! A rule is read whole from a [`Tree`], its list part and its named keys
//! together, and a broken one is refused with its number, its key and, for a
//! part or a slot, what was probably meant (Ruling 13).
//! `tests::an_unknown_part_is_refused_with_its_rule_number_and_a_suggestion`.
//!
//! [`Rules`] resolves a part's three slots from what a frame knows of it, the
//! later rule winning across origins (style, material, expansion, user), and
//! says up front which facts any rule reads; [`tier`] decides from what a
//! bound plan reads whether this build runs it (Ruling 14).
//! `tests::a_users_rule_beats_the_styles`,
//! `tests::tiers_follow_what_the_plan_reads_and_the_source`.

use solium_effects::spec::Value;
use solium_effects::stage::Reads;

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

    /// Whether this rule's part is the one a frame names: the same kind, and
    /// the same name, or for a layer surface a namespace its glob matches.
    /// `tests::each_whole_part_reaches_only_itself`,
    /// `tests::a_layer_rule_reaches_only_layers_of_its_name`,
    /// `tests::a_surface_or_layer_shell_rule_reaches_only_its_own`.
    pub(crate) fn is(&self, part: PartRef<'_>) -> bool {
        match (self, part) {
            (Self::Pane, PartRef::Pane)
            | (Self::Client, PartRef::Client)
            | (Self::Popup, PartRef::Popup) => true,
            (Self::Layer(name), PartRef::Layer(named))
            | (Self::Region(name), PartRef::Region(named))
            | (Self::Surface(name), PartRef::Surface(named)) => name == named,
            (Self::LayerShell(glob), PartRef::LayerShell(namespace)) => glob.matches(namespace),
            _ => false,
        }
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

/// Which rule fills a slot: its origin, its place in that origin's list, and
/// the generation of the list it was read from, so a key from a list since
/// replaced finds nothing of the new one by accident.
/// `tests::a_key_finds_its_rule_in_its_origin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuleKey {
    pub(crate) origin: Origin,
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

/// One part of a pane or a surface, as a frame names it to resolve its
/// slots: [`Part`] borrowed, a layer surface by its namespace.
/// `tests::a_layer_rule_reaches_only_layers_of_its_name`,
/// `tests::a_surface_or_layer_shell_rule_reaches_only_its_own`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PartRef<'a> {
    Pane,
    Client,
    Popup,
    Layer(&'a str),
    Region(&'a str),
    Surface(&'a str),
    LayerShell(&'a str),
}

/// What a frame knows of a window, a scripted surface or a layer surface,
/// which a rule's `match` is checked against.
/// `tests::every_match_key_is_checked_against_its_fact`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Facts<'a> {
    pub(crate) app_id: &'a str,
    pub(crate) title: &'a str,
    pub(crate) focused: bool,
    pub(crate) fullscreen: bool,
    pub(crate) monitor: &'a str,
    pub(crate) style: &'a str,
    pub(crate) surface: &'a str,
    pub(crate) layer_shell: &'a str,
}

/// The facts any rule reads that cost something to gather (a title is a
/// `String` per pane, a monitor an output scan, an app id a lookup); the
/// booleans and the style are free, so a frame gathers only these.
/// `tests::with_no_rules_nothing_resolves_and_no_fact_is_asked`,
/// `tests::the_facts_asked_are_every_origins_keys`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Keys {
    pub(crate) app_id: bool,
    pub(crate) title: bool,
    pub(crate) monitor: bool,
}

/// One part's three slots, each the rule that fills it, if any.
/// `tests::the_later_rule_for_one_part_and_slot_wins`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) behind: Option<RuleKey>,
    pub(crate) front: Option<RuleKey>,
    pub(crate) replace: Option<RuleKey>,
}

impl Resolved {
    /// `tests::a_key_finds_its_rule_in_its_origin`.
    pub(crate) fn get(&self, slot: Slot) -> Option<RuleKey> {
        match slot {
            Slot::Behind => self.behind,
            Slot::Front => self.front,
            Slot::Replace => self.replace,
        }
    }

    /// `tests::with_no_rules_nothing_resolves_and_no_fact_is_asked`.
    pub(crate) fn is_empty(&self) -> bool {
        self.behind.is_none() && self.front.is_none() && self.replace.is_none()
    }
}

impl Match {
    /// Every key given matches its fact; a key not given matches anything.
    /// `tests::every_match_key_is_checked_against_its_fact`.
    pub(crate) fn accepts(&self, facts: &Facts<'_>) -> bool {
        let word = |glob: Option<&Glob>, fact: &str| glob.is_none_or(|glob| glob.matches(fact));
        let flag = |wanted: Option<bool>, fact: bool| wanted.is_none_or(|wanted| wanted == fact);
        word(self.app_id.as_ref(), facts.app_id)
            && word(self.title.as_ref(), facts.title)
            && flag(self.focused, facts.focused)
            && flag(self.fullscreen, facts.fullscreen)
            && word(self.monitor.as_ref(), facts.monitor)
            && word(self.style.as_ref(), facts.style)
            && word(self.surface.as_ref(), facts.surface)
            && word(self.layer_shell.as_ref(), facts.layer_shell)
    }
}

/// Every origin's rules but the style's, which ride on each pane's
/// `Decoration` (Task 15) and are passed in.
/// `tests::a_users_rule_beats_the_styles`.
#[derive(Debug, Default)]
pub(crate) struct Rules {
    /// Material, expansion and user, in that order; the style's ride on the
    /// pane. `tests::a_key_finds_its_rule_in_its_origin`.
    lists: [Vec<Rule>; 3],
    generation: u32,
}

impl Rules {
    /// `tests::a_users_rule_beats_the_styles`.
    pub(crate) fn new(
        material: Vec<Rule>,
        expansion: Vec<Rule>,
        user: Vec<Rule>,
        generation: u32,
    ) -> Self {
        Self {
            lists: [material, expansion, user],
            generation,
        }
    }

    /// `tests::with_no_rules_nothing_resolves_and_no_fact_is_asked`.
    pub(crate) fn is_empty(&self) -> bool {
        self.lists.iter().all(Vec::is_empty)
    }

    /// The user's rules, as `sol.effects` gave them: what a rebind after the
    /// formats probe binds again.
    /// `state::tests::the_formats_probe_rebinds_the_rules_after_the_frame`.
    pub(crate) fn user(&self) -> &[Rule] {
        let [_, _, user] = &self.lists;
        user
    }

    /// Each origin's list with its generation, in the order a later rule
    /// wins: style, material, expansion, user (Ruling 13).
    /// `tests::a_users_rule_beats_the_styles`.
    fn origins<'a>(
        &'a self,
        style: &'a [Rule],
        style_generation: u32,
    ) -> impl Iterator<Item = (Origin, u32, &'a [Rule])> {
        let [material, expansion, user] = &self.lists;
        [
            (Origin::Style, style_generation, style),
            (Origin::Material, self.generation, material.as_slice()),
            (Origin::Expansion, self.generation, expansion.as_slice()),
            (Origin::User, self.generation, user.as_slice()),
        ]
        .into_iter()
    }

    /// The facts any rule reads, so a frame gathers only those.
    /// `tests::with_no_rules_nothing_resolves_and_no_fact_is_asked`,
    /// `tests::the_facts_asked_are_every_origins_keys`.
    pub(crate) fn uses(&self, style: &[Rule]) -> Keys {
        let mut keys = Keys::default();
        for (_, _, list) in self.origins(style, 0) {
            for rule in list {
                keys.app_id |= rule.matches.app_id.is_some();
                keys.title |= rule.matches.title.is_some();
                keys.monitor |= rule.matches.monitor.is_some();
            }
        }
        keys
    }

    /// The last matching rule per slot, for one part; `effect = false` empties
    /// the slot it wins. A walk of each origin's list: an index by part kind
    /// waits until the trace's `prep_us` shows the walk.
    /// `tests::the_later_rule_for_one_part_and_slot_wins`,
    /// `tests::effect_false_after_a_chain_empties_the_slot`,
    /// `tests::a_rule_that_stops_matching_lets_the_earlier_one_back`.
    pub(crate) fn resolve(
        &self,
        style: &[Rule],
        style_generation: u32,
        part: PartRef<'_>,
        facts: &Facts<'_>,
    ) -> Resolved {
        let mut resolved = Resolved::default();
        for (origin, generation, list) in self.origins(style, style_generation) {
            for (index, rule) in list.iter().enumerate() {
                if !rule.part.is(part) || !rule.matches.accepts(facts) {
                    continue;
                }
                let key = (rule.fill != Fill::Off).then_some(RuleKey {
                    origin,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                    generation,
                });
                match rule.slot {
                    Slot::Behind => resolved.behind = key,
                    Slot::Front => resolved.front = key,
                    Slot::Replace => resolved.replace = key,
                }
            }
        }
        resolved
    }

    /// The rule a key names, the style's from `style`: the walk reads a
    /// slot's for its mask (`render::cut_by_shape`).
    /// `tests::a_key_finds_its_rule_in_its_origin`.
    pub(crate) fn rule<'a>(&'a self, style: &'a [Rule], key: RuleKey) -> Option<&'a Rule> {
        let [material, expansion, user] = &self.lists;
        let list = match key.origin {
            Origin::Style => style,
            Origin::Material => material,
            Origin::Expansion => expansion,
            Origin::User => user,
        };
        list.get(usize::try_from(key.index).ok()?)
    }

    /// Every effect any non-style rule names, each once: what the host must
    /// want. `tests::the_rules_name_the_effects_the_host_must_want`.
    pub(crate) fn effects(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .lists
            .iter()
            .flatten()
            .filter_map(|rule| match &rule.fill {
                Fill::Chain(links) => Some(links.iter().map(|link| link.effect.clone())),
                Fill::Off => None,
            })
            .flatten()
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// Which tier a bound chain runs in (Ruling 14).
/// `tests::tiers_follow_what_the_plan_reads_and_the_source`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tier {
    /// T0 generated: reads nothing of the frame (only `shape` and `state:`).
    /// Not \[16\]'s T0 *inline*, a per-surface program reading its own pixels
    /// with no capture, which is X1.4b's (Ruling 14): such an effect is `Own`.
    /// `tests::tiers_follow_what_the_plan_reads_and_the_source`.
    Generated,
    /// T1: a capture of the part.
    Own,
    /// T2: the sharp wallpaper and bottom layers, memoised.
    Xray,
    /// T3: the frame drawn so far.
    Live,
}

/// The tier from what the bound plan reads and the rule's `source`: a
/// backdrop is read from xray unless `source` rebinds it, the part's own
/// pixels or the picture from before are a capture of the part, and the
/// highest read decides. `tests::tiers_follow_what_the_plan_reads_and_the_source`,
/// `tests::the_old_picture_is_t1_and_the_higher_read_decides`.
pub(crate) fn tier(reads: Reads, source: Option<Source>) -> Tier {
    let backdrop = match (reads.backdrop, source) {
        (false, _) => None,
        (true, Some(Source::Own)) => Some(Tier::Own),
        (true, None | Some(Source::Xray)) => Some(Tier::Xray),
        (true, Some(Source::Live | Source::Auto)) => Some(Tier::Live),
    };
    let own = (reads.own || reads.old).then_some(Tier::Own);
    backdrop
        .into_iter()
        .chain(own)
        .max()
        .unwrap_or(Tier::Generated)
}

/// Whether this build runs a tier; the error names what brings it.
/// `tests::tiers_follow_what_the_plan_reads_and_the_source`.
pub(crate) fn runnable(tier: Tier) -> Result<(), &'static str> {
    match tier {
        Tier::Generated | Tier::Own => Ok(()),
        Tier::Xray => Err(
            "reads the backdrop from xray, which arrives with X2.1 (FX3); give `source = \"self\"` to read the part's own pixels",
        ),
        Tier::Live => Err("reads the live backdrop, which arrives with X4.1 (FX6)"),
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

    fn user(lua: &str) -> super::Rules {
        super::Rules::new(
            Vec::new(),
            Vec::new(),
            parse(&rules(lua)).expect("parses"),
            1,
        )
    }

    fn mpv() -> super::Facts<'static> {
        super::Facts {
            app_id: "mpv",
            title: "a film",
            focused: true,
            monitor: "DP-1",
            style: "top",
            ..super::Facts::default()
        }
    }

    #[test]
    fn the_later_rule_for_one_part_and_slot_wins() {
        let rules = user(
            r#"{ { match = "*", part = "client", slot = "behind", effect = "a" },
                              { match = { app_id = "mpv" }, part = "client", slot = "behind", effect = "b" } }"#,
        );
        let resolved = rules.resolve(&[], 0, super::PartRef::Client, &mpv());
        assert_eq!(resolved.behind.map(|key| key.index), Some(1));
        assert!(
            resolved.front.is_none() && resolved.replace.is_none(),
            "a rule for another slot leaves this one alone"
        );
    }

    #[test]
    fn effect_false_after_a_chain_empties_the_slot() {
        let rules = user(
            r#"{ { match = "*", part = "client", slot = "behind", effect = "a" },
                              { match = { app_id = "mpv" }, part = "client", slot = "behind", effect = false } }"#,
        );
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Client, &mpv())
                .behind
                .is_none()
        );
    }

    /// **Your rule beats the style's**: origins in the order style,
    /// material, expansion, user.
    #[test]
    fn a_users_rule_beats_the_styles() {
        let style = parse(&rules(
            r#"{ { match = "*", part = "client", slot = "behind", effect = "shadow" } }"#,
        ))
        .expect("parses");
        let rules =
            user(r#"{ { match = "*", part = "client", slot = "behind", effect = "glow" } }"#);
        let key = rules
            .resolve(&style, 7, super::PartRef::Client, &mpv())
            .behind
            .expect("resolved");
        assert_eq!(key.origin, super::Origin::User);
        let none = super::Rules::default();
        assert_eq!(
            none.resolve(&style, 7, super::PartRef::Client, &mpv())
                .behind
                .map(|key| (key.origin, key.generation)),
            Some((super::Origin::Style, 7))
        );
    }

    #[test]
    fn a_rule_that_stops_matching_lets_the_earlier_one_back() {
        let rules = user(
            r#"{ { match = "*", part = "client", slot = "behind", effect = "a" },
                              { match = { focused = true }, part = "client", slot = "behind", effect = "b" } }"#,
        );
        assert_eq!(
            rules
                .resolve(&[], 0, super::PartRef::Client, &mpv())
                .behind
                .map(|key| key.index),
            Some(1)
        );
        let blurred = super::Facts {
            focused: false,
            ..mpv()
        };
        assert_eq!(
            rules
                .resolve(&[], 0, super::PartRef::Client, &blurred)
                .behind
                .map(|key| key.index),
            Some(0)
        );
    }

    /// **With no rules nothing resolves and no fact is asked.**
    #[test]
    fn with_no_rules_nothing_resolves_and_no_fact_is_asked() {
        let rules = super::Rules::default();
        assert!(rules.is_empty());
        assert_eq!(rules.uses(&[]), super::Keys::default());
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Client, &mpv())
                .is_empty()
        );
        let titled = user(
            r#"{ { match = { title = "x*" }, part = "client", slot = "front", effect = false } }"#,
        );
        assert!(titled.uses(&[]).title && !titled.uses(&[]).app_id);
    }

    #[test]
    fn a_layer_rule_reaches_only_layers_of_its_name() {
        let rules = user(
            r#"{ { match = "*", part = "layer:shadow", slot = "replace", effect = "soft" } }"#,
        );
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Layer("shadow"), &mpv())
                .replace
                .is_some()
        );
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Layer("bar"), &mpv())
                .replace
                .is_none()
        );
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Client, &mpv())
                .replace
                .is_none()
        );
    }

    #[test]
    fn tiers_follow_what_the_plan_reads_and_the_source() {
        use super::{Source, Tier, runnable, tier};
        use solium_effects::stage::Reads;
        let backdrop = Reads {
            backdrop: true,
            ..Reads::default()
        };
        assert_eq!(
            tier(backdrop, Some(Source::Own)),
            Tier::Own,
            "a backdrop rebound to self is T1"
        );
        assert_eq!(
            tier(backdrop, None),
            Tier::Xray,
            "the default source is xray"
        );
        assert_eq!(tier(backdrop, Some(Source::Live)), Tier::Live);
        assert_eq!(tier(backdrop, Some(Source::Auto)), Tier::Live);
        assert_eq!(
            tier(
                Reads {
                    shape: true,
                    ..Reads::default()
                },
                None
            ),
            Tier::Generated,
            "reading nothing of the frame is T0 generated"
        );
        assert_eq!(
            tier(
                Reads {
                    own: true,
                    ..Reads::default()
                },
                None
            ),
            Tier::Own,
            "reading its own pixels is T1 until X1.4b's inline tier"
        );
        assert!(runnable(Tier::Generated).is_ok() && runnable(Tier::Own).is_ok());
        assert!(runnable(Tier::Xray).expect_err("refused").contains("X2.1"));
        assert!(runnable(Tier::Live).expect_err("refused").contains("X4.1"));
    }

    /// The picture from before a resize (`old`) is the part's own too, and a
    /// backdrop read beside it keeps the higher tier.
    #[test]
    fn the_old_picture_is_t1_and_the_higher_read_decides() {
        use super::{Source, Tier, tier};
        use solium_effects::stage::Reads;
        let old = Reads {
            old: true,
            ..Reads::default()
        };
        assert_eq!(tier(old, None), Tier::Own);
        assert_eq!(
            tier(old, Some(Source::Xray)),
            Tier::Own,
            "nothing reads the backdrop"
        );
        let both = Reads {
            own: true,
            backdrop: true,
            ..Reads::default()
        };
        assert_eq!(tier(both, None), Tier::Xray);
        assert_eq!(tier(both, Some(Source::Own)), Tier::Own);
        assert_eq!(tier(both, Some(Source::Xray)), Tier::Xray);
        assert_eq!(tier(Reads::default(), Some(Source::Live)), Tier::Generated);
    }

    /// `pane`, `client` and `popup` each reach their own part alone, and the
    /// titlebar only the titlebar.
    #[test]
    fn each_whole_part_reaches_only_itself() {
        use super::PartRef;
        let rules = user(
            r#"{ { match = "*", part = "pane", slot = "behind", effect = "a" },
                 { match = "*", part = "client", slot = "front", effect = "b" },
                 { match = "*", part = "popup", slot = "replace", effect = "c" },
                 { match = "*", part = "region:titlebar", slot = "front", effect = "d" } }"#,
        );
        let slots = |part| {
            let resolved = rules.resolve(&[], 0, part, &mpv());
            [resolved.behind, resolved.front, resolved.replace].map(|key| key.map(|key| key.index))
        };
        assert_eq!(slots(PartRef::Pane), [Some(0), None, None]);
        assert_eq!(slots(PartRef::Client), [None, Some(1), None]);
        assert_eq!(slots(PartRef::Popup), [None, None, Some(2)]);
        assert_eq!(slots(PartRef::Region("titlebar")), [None, Some(3), None]);
        assert_eq!(slots(PartRef::Layer("titlebar")), [None, None, None]);
    }

    /// A scripted surface's rule reaches that surface alone, and a layer
    /// surface's rule every namespace its glob matches; neither reaches a
    /// window's part.
    #[test]
    fn a_surface_or_layer_shell_rule_reaches_only_its_own() {
        let rules = user(
            r#"{ { match = "*", part = "surface:bar", slot = "behind", effect = "glow" },
                 { match = "*", part = "layer_shell:waybar*", slot = "front", effect = "glow" } }"#,
        );
        let bar = super::Facts {
            surface: "bar",
            ..super::Facts::default()
        };
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Surface("bar"), &bar)
                .behind
                .is_some()
        );
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Surface("dock"), &bar)
                .is_empty()
        );
        for (namespace, reached) in [("waybar", true), ("waybar-top", true), ("mako", false)] {
            let facts = super::Facts {
                layer_shell: namespace,
                ..super::Facts::default()
            };
            assert_eq!(
                rules
                    .resolve(&[], 0, super::PartRef::LayerShell(namespace), &facts)
                    .front
                    .is_some(),
                reached,
                "{namespace}"
            );
        }
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Pane, &mpv())
                .is_empty()
        );
        assert!(
            rules
                .resolve(&[], 0, super::PartRef::Region("titlebar"), &mpv())
                .is_empty()
        );
    }

    /// Every match key is checked against its own fact; a key not given
    /// matches anything.
    #[test]
    fn every_match_key_is_checked_against_its_fact() {
        use super::{Facts, PartRef};
        let surface = |name| Facts {
            surface: name,
            ..Facts::default()
        };
        let namespace = |name| Facts {
            layer_shell: name,
            ..Facts::default()
        };
        for (matched, part, part_ref, refused, accepted) in [
            (
                r#"{ app_id = "mpv" }"#,
                "client",
                PartRef::Client,
                Facts {
                    app_id: "vlc",
                    ..mpv()
                },
                mpv(),
            ),
            (
                r#"{ title = "a f*" }"#,
                "client",
                PartRef::Client,
                Facts {
                    title: "a book",
                    ..mpv()
                },
                mpv(),
            ),
            (
                r#"{ focused = true }"#,
                "client",
                PartRef::Client,
                Facts {
                    focused: false,
                    ..mpv()
                },
                mpv(),
            ),
            (
                r#"{ fullscreen = false }"#,
                "client",
                PartRef::Client,
                Facts {
                    fullscreen: true,
                    ..mpv()
                },
                mpv(),
            ),
            (
                r#"{ monitor = "DP-*" }"#,
                "client",
                PartRef::Client,
                Facts {
                    monitor: "HDMI-A-1",
                    ..mpv()
                },
                mpv(),
            ),
            (
                r#"{ style = "top" }"#,
                "client",
                PartRef::Client,
                Facts {
                    style: "none",
                    ..mpv()
                },
                mpv(),
            ),
            (
                r#"{ surface = "bar" }"#,
                "surface:bar",
                PartRef::Surface("bar"),
                surface("dock"),
                surface("bar"),
            ),
            (
                r#"{ layer_shell = "waybar" }"#,
                "layer_shell:*",
                PartRef::LayerShell("waybar"),
                namespace("mako"),
                namespace("waybar"),
            ),
        ] {
            let rules = user(&format!(
                r#"{{ {{ match = {matched}, part = "{part}", slot = "behind", effect = "a" }} }}"#
            ));
            assert!(
                rules.resolve(&[], 0, part_ref, &refused).is_empty(),
                "{matched} refuses {refused:?}"
            );
            assert!(
                rules.resolve(&[], 0, part_ref, &accepted).behind.is_some(),
                "{matched} accepts {accepted:?}"
            );
        }
    }

    /// A key finds its rule in its origin's list, the style's passed in;
    /// `get` reads a slot by name.
    #[test]
    fn a_key_finds_its_rule_in_its_origin() {
        let style = parse(&rules(
            r#"{ { match = "*", part = "client", slot = "front", effect = "shadow" } }"#,
        ))
        .expect("parses");
        let rules = user(
            r#"{ { match = "*", part = "pane", slot = "behind", effect = "a" },
                 { match = "*", part = "client", slot = "behind", effect = "glow" } }"#,
        );
        let resolved = rules.resolve(&style, 3, super::PartRef::Client, &mpv());
        let front = resolved.get(Slot::Front).expect("the style's");
        let behind = resolved.get(Slot::Behind).expect("yours");
        assert_eq!(resolved.get(Slot::Replace), None);
        let named = |key| match &rules.rule(&style, key).expect("found").fill {
            Fill::Chain(links) => links[0].effect.clone(),
            Fill::Off => String::new(),
        };
        assert_eq!(
            (named(front), named(behind)),
            ("shadow".to_owned(), "glow".to_owned())
        );
        assert_eq!(
            (front.origin, front.index, front.generation),
            (super::Origin::Style, 0, 3)
        );
        assert_eq!(
            (behind.origin, behind.index, behind.generation),
            (super::Origin::User, 1, 1)
        );
        let gone = super::RuleKey { index: 9, ..behind };
        assert!(rules.rule(&style, gone).is_none());
        let material = super::RuleKey {
            origin: super::Origin::Material,
            index: 0,
            generation: 1,
        };
        assert!(
            rules.rule(&style, material).is_none(),
            "material is empty in FX2"
        );
    }

    /// The effects the host must want: every link of every non-style rule,
    /// each once; an emptied slot names none, and the style's ride on the
    /// pane and are wanted when it is applied (Ruling 15).
    #[test]
    fn the_rules_name_the_effects_the_host_must_want() {
        let rules = user(
            r#"{ { match = "*", part = "client", slot = "behind", effect = { { "kawase", source = "self" }, { "tint" } } },
                 { match = "*", part = "pane", slot = "front", effect = "tint" },
                 { match = "*", part = "popup", slot = "front", effect = false },
                 { match = "*", part = "layer:bar", slot = "replace", effect = "frost" } }"#,
        );
        assert_eq!(rules.effects(), ["frost", "kawase", "tint"]);
        assert!(super::Rules::default().effects().is_empty());
    }

    /// The facts a style's rules read are asked too, each key on its own.
    #[test]
    fn the_facts_asked_are_every_origins_keys() {
        let style = parse(&rules(
            r#"{ { match = { monitor = "DP-*" }, part = "client", slot = "front", effect = false } }"#,
        ))
        .expect("parses");
        assert_eq!(
            super::Rules::default().uses(&style),
            super::Keys {
                monitor: true,
                ..super::Keys::default()
            }
        );
        let rules = user(
            r#"{ { match = { app_id = "mpv", focused = true, fullscreen = false, style = "top" }, part = "client", slot = "front", effect = false } }"#,
        );
        assert_eq!(
            rules.uses(&[]),
            super::Keys {
                app_id: true,
                ..super::Keys::default()
            },
            "the booleans and the style are free"
        );
        assert!(!rules.is_empty());
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
