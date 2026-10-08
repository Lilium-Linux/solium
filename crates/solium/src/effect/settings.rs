//! The engine's own numbers and policies, each a key under `effects` with
//! today's value as its default (Ruling 28): what an effect's Lua may spend
//! as it loads, how many params an effect may pack, and what a `sol.present`
//! geometry draws when it cannot be drawn.
//! `tests::the_engines_keys_read_with_their_defaults_and_bounds`.

use std::time::Duration;

use super::tree::Tree;

/// What `sol.effects` reads: an unknown key is refused by name, so a typo
/// never means "the default". Task 30 adds `timing`, Task 27b `geometry` (and
/// `mesh_ms` and `revive` to `sandbox`'s list in [`parse`]), Task 31 `on`.
/// `tests::the_engines_keys_read_with_their_defaults_and_bounds`.
pub(crate) const KEYS: &[&str] = &["rules", "sandbox", "limits", "present"];

/// Everything under `effects` that is not a rule or a transition.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Settings {
    pub(crate) sandbox: Caps,
    pub(crate) limits: Limits,
    pub(crate) present: Present,
}

/// `effects.sandbox`: what one effect's Lua may spend as it loads.
/// `sandbox::tests::the_load_budget_is_configurable`,
/// `sandbox::tests::the_memory_cap_is_configurable`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Caps {
    /// A load-time call's budget (`effect.lua`, `stages`, `reach`, `bleed`,
    /// a style's `effects.lua`), and the one the checks at load share.
    pub(crate) load: Duration,
    /// The bytes a state may hold.
    pub(crate) memory: usize,
}

impl Caps {
    /// 100 ms and 16 MiB: the defaults, as a constant, so a `static` can
    /// start from them (`style::set_caps`).
    pub(crate) const DEFAULT: Self = Self {
        load: Duration::from_millis(100),
        memory: 16 << 20,
    };
}

impl Default for Caps {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `effects.limits`: how much an effect may ask of the engine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Limits {
    /// Numeric params a geometry or pixels effect may pack.
    /// `state::tests::real_client::a_geometry_effect_with_twelve_params_presents_under_a_raised_limit`.
    pub(crate) params: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { params: 8 }
    }
}

/// `effects.present`: a `sol.present` geometry's policies, each also a
/// `deform` key.
/// `state::tests::real_client::effects_present_is_the_default_a_deform_overrides`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Present {
    pub(crate) failed: PresentFailed,
    pub(crate) on_reload: PresentReload,
}

/// What a present whose mesh is refused, or whose folder is missing, draws.
/// `render::tests::a_refused_present_follows_its_failed_policy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PresentFailed {
    /// The window undeformed: today's.
    #[default]
    Flat,
    /// Nothing of it.
    Hide,
}

impl PresentFailed {
    /// The words a configuration or a deform says it with, the default
    /// first.
    pub(crate) const WORDS: [&str; 2] = ["flat", "hide"];

    pub(crate) fn from_word(word: &str) -> Option<Self> {
        match word {
            "flat" => Some(Self::Flat),
            "hide" => Some(Self::Hide),
            _ => None,
        }
    }
}

/// What a present does when its folder is reloaded mid-flight.
/// `state::tests::real_client::a_reload_mid_present_follows_on_reload`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PresentReload {
    /// Drawn flat for the rest of the transform: today's.
    #[default]
    Flat,
    /// On the version it began with, to the end of the transform.
    Keep,
}

impl PresentReload {
    /// The words a configuration or a deform says it with, the default
    /// first.
    pub(crate) const WORDS: [&str; 2] = ["flat", "keep"];

    pub(crate) fn from_word(word: &str) -> Option<Self> {
        match word {
            "flat" => Some(Self::Flat),
            "keep" => Some(Self::Keep),
            _ => None,
        }
    }
}

/// What is at `path` (`"sandbox.load_ms"`) inside `options`.
fn at<'a>(options: &'a Tree, path: &str) -> Option<&'a Tree> {
    let mut at = Some(options);
    for key in path.split('.') {
        at = at.and_then(|tree| tree.field(key));
    }
    at
}

/// A number key at `path`, its default when absent, refused outside
/// `[low, high]` with the key and the bounds named.
/// `tests::the_engines_keys_read_with_their_defaults_and_bounds`.
pub(crate) fn bounded(
    options: &Tree,
    path: &str,
    default: f64,
    low: f64,
    high: f64,
) -> Result<f64, String> {
    let value = match at(options, path) {
        None => return Ok(default),
        Some(Tree::Number(number)) => *number,
        #[expect(
            clippy::cast_precision_loss,
            reason = "a configured number, far below 2^52"
        )]
        Some(Tree::Int(int)) => *int as f64,
        Some(_) => f64::NAN,
    };
    if (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(format!("`effects.{path}` is a number from {low} to {high}"))
    }
}

/// `"flat" or "hide"`: the words a key takes, as a message says them.
pub(crate) fn said(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| format!("\"{word}\""))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// A word key at `path`, one of `words`, the first when absent.
/// `tests::the_engines_keys_read_with_their_defaults_and_bounds`.
pub(crate) fn word<'a>(options: &Tree, path: &str, words: &[&'a str]) -> Result<&'a str, String> {
    match at(options, path) {
        None => words
            .first()
            .copied()
            .ok_or_else(|| format!("`effects.{path}` has no words")),
        Some(Tree::Text(text)) => words
            .iter()
            .copied()
            .find(|word| word == text)
            .ok_or_else(|| format!("`effects.{path}` is {}", said(words))),
        Some(_) => Err(format!("`effects.{path}` is {}", said(words))),
    }
}

/// Unknown keys of a table, refused by name with the nearest known one.
fn known(options: &Tree, path: &str, keys: &[&str]) -> Result<(), String> {
    let Tree::Table { fields, .. } = options else {
        return Ok(());
    };
    match fields.keys().find(|key| !keys.contains(&key.as_str())) {
        None => Ok(()),
        Some(key) => {
            let meant = solium_effects::spec::nearest(key, keys)
                .map(|meant| format!("; did you mean `{meant}`?"))
                .unwrap_or_default();
            Err(format!("`effects{path}` has no key `{key}`{meant}"))
        }
    }
}

/// The tables under `effects` whose keys [`parse`] reads, each with its
/// keys. Task 27b adds `mesh_ms` and `revive` to `sandbox`'s, and
/// `geometry` with its own.
const TABLES: [(&str, &[&str]); 3] = [
    ("sandbox", &["load_ms", "memory_mib"]),
    ("limits", &["params"]),
    ("present", &["failed", "on_reload"]),
];

/// `sol.effects`' options as a [`Tree`], a number that is not finite kept
/// where [`Tree::from_lua`] drops it (directly under `effects`, and in
/// [`TABLES`]), so `load_ms = 1/0` is refused for its bounds rather than
/// read as the default.
/// `tests::the_engines_keys_read_with_their_defaults_and_bounds`.
pub(crate) fn tree_of(options: &mlua::Table) -> mlua::Result<Tree> {
    let mut tree =
        Tree::from_lua(&mlua::Value::Table(options.clone()))?.unwrap_or_else(|| Tree::Table {
            list: Vec::new(),
            fields: std::collections::BTreeMap::new(),
        });
    if let Tree::Table { fields, .. } = &mut tree {
        keep_non_finite(options, fields)?;
        for (table, _) in TABLES {
            if let Ok(mlua::Value::Table(inner)) = options.raw_get::<mlua::Value>(table)
                && let Some(Tree::Table { fields, .. }) = fields.get_mut(table)
            {
                keep_non_finite(&inner, fields)?;
            }
        }
    }
    Ok(tree)
}

/// Every named number of `table` that is not finite, into `fields`.
fn keep_non_finite(
    table: &mlua::Table,
    fields: &mut std::collections::BTreeMap<String, Tree>,
) -> mlua::Result<()> {
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        if let (mlua::Value::String(key), mlua::Value::Number(number)) = pair?
            && !number.is_finite()
        {
            fields.insert(key.to_str()?.to_owned(), Tree::Number(number));
        }
    }
    Ok(())
}

/// `sol.effects`' options, beside `rules`: every key checked, each table's
/// key a table, each number within its bounds and each word one of its
/// words, or the whole set refused naming the key.
/// `tests::the_engines_keys_read_with_their_defaults_and_bounds`.
pub(crate) fn parse(options: &Tree) -> Result<Settings, String> {
    known(options, "", KEYS)?;
    for (table, keys) in TABLES {
        match options.field(table) {
            None => {}
            Some(tree @ Tree::Table { .. }) => known(tree, &format!(".{table}"), keys)?,
            // Anything else is refused, not read as the defaults.
            Some(_) => {
                let keys: Vec<String> = keys.iter().map(|key| format!("`{key}`")).collect();
                return Err(format!(
                    "`effects.{table}` is a table of {}",
                    keys.join(", ")
                ));
            }
        }
    }
    let load = bounded(options, "sandbox.load_ms", 100.0, 10.0, 5000.0)?;
    let memory = bounded(options, "sandbox.memory_mib", 16.0, 1.0, 512.0)?;
    // At most `geometry::PARAMS_MAX`, the inline array's length: a
    // representation's size, not a behaviour (Ruling 28).
    #[expect(clippy::cast_precision_loss, reason = "64")]
    let most = crate::effect::geometry::PARAMS_MAX as f64;
    let params = bounded(options, "limits.params", 8.0, 1.0, most)?;
    let failed = PresentFailed::from_word(word(options, "present.failed", &PresentFailed::WORDS)?)
        .unwrap_or_default();
    let on_reload =
        PresentReload::from_word(word(options, "present.on_reload", &PresentReload::WORDS)?)
            .unwrap_or_default();
    // Whole numbers in Lua's use; a fraction is cut. Both are within the
    // bounds checked above.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "validated above: within bounds, and a fraction is cut"
    )]
    let (memory, params) = ((memory as usize) << 20, params as usize);
    Ok(Settings {
        sandbox: Caps {
            load: Duration::from_secs_f64(load / 1000.0),
            memory,
        },
        limits: Limits { params },
        present: Present { failed, on_reload },
    })
}

#[cfg(test)]
mod tests {
    use super::Tree;

    /// `sol.effects`' options as it reads them ([`super::tree_of`]).
    fn tree(lua: &str) -> Tree {
        let state = mlua::Lua::new();
        let table: mlua::Table = state.load(lua).eval().expect("the test's Lua");
        super::tree_of(&table).expect("readable")
    }

    /// **The engine's keys read with their defaults and within their
    /// bounds** (Ruling 28): a set naming none is today's numbers; a number
    /// outside its bounds (one that is not finite included), a word outside
    /// its words, a table's key given anything but a table, and an unknown
    /// key are each refused, naming the key.
    #[test]
    fn the_engines_keys_read_with_their_defaults_and_bounds() {
        let none = super::parse(&tree("{ rules = {} }")).expect("parses");
        assert_eq!(none, super::Settings::default());
        assert_eq!(
            (none.sandbox.load, none.sandbox.memory, none.limits.params),
            (std::time::Duration::from_millis(100), 16 << 20, 8),
            "today's numbers"
        );
        assert_eq!(
            (none.present.failed, none.present.on_reload),
            (super::PresentFailed::Flat, super::PresentReload::Flat)
        );
        let set = super::parse(&tree(
            "{ sandbox = { load_ms = 500, memory_mib = 64 }, limits = { params = 16 }, present = { failed = 'hide', on_reload = 'keep' } }",
        ))
        .expect("parses");
        assert_eq!(
            (set.sandbox.load, set.sandbox.memory, set.limits.params),
            (std::time::Duration::from_millis(500), 64 << 20, 16)
        );
        assert_eq!(
            (set.present.failed, set.present.on_reload),
            (super::PresentFailed::Hide, super::PresentReload::Keep)
        );
        for (lua, says) in [
            (
                "{ sandbox = { load_ms = 5 } }",
                "`effects.sandbox.load_ms` is a number from 10 to 5000",
            ),
            (
                "{ sandbox = { memory_mib = 1024 } }",
                "`effects.sandbox.memory_mib` is a number from 1 to 512",
            ),
            (
                "{ limits = { params = 65 } }",
                "`effects.limits.params` is a number from 1 to 64",
            ),
            (
                "{ limits = { params = 'many' } }",
                "`effects.limits.params` is a number from 1 to 64",
            ),
            (
                "{ present = { failed = 'fade' } }",
                "`effects.present.failed` is \"flat\" or \"hide\"",
            ),
            (
                "{ present = { on_reload = 'switch' } }",
                "`effects.present.on_reload` is \"flat\" or \"keep\"",
            ),
            // No "did you mean": `load` is 3 edits from `load_ms`, past
            // `nearest`'s 2.
            (
                "{ sandbox = { load = 500 } }",
                "`effects.sandbox` has no key `load`",
            ),
            ("{ sandbx = {} }", "sandbox"),
            // Not a table: refused, not read as the defaults.
            ("{ present = 'hide' }", "`effects.present` is a table"),
            ("{ sandbox = 5 }", "`effects.sandbox` is a table"),
            ("{ limits = true }", "`effects.limits` is a table"),
            ("{ sandbox = 1/0 }", "`effects.sandbox` is a table"),
            // Not finite: refused for its bounds, not dropped for the
            // default.
            (
                "{ sandbox = { load_ms = 1/0 } }",
                "`effects.sandbox.load_ms` is a number from 10 to 5000",
            ),
            (
                "{ limits = { params = 0/0 } }",
                "`effects.limits.params` is a number from 1 to 64",
            ),
            (
                "{ present = { failed = -1/0 } }",
                "`effects.present.failed` is \"flat\" or \"hide\"",
            ),
        ] {
            let refused = super::parse(&tree(lua)).expect_err(lua);
            assert!(refused.contains(says), "{lua}: {refused}");
        }
    }
}
