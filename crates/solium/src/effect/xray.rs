//! xray (Ruling F29): the sharp picture of the layers below a part's band,
//! per monitor, from which every T2 effect reads its `backdrop`. Its source
//! is a parameter, so FX6's lock xray is a second one (Ruling F52).
//!
//! **The live backdrop is a cut too** (#209): [`Cut::Beneath`] holds
//! everything drawn strictly below one part on its monitor -- the windows
//! under a bar as well as the wallpaper -- and a rule with `source = "live"`
//! (or `"auto"`) reads it as `backdrop` where a T2 rule reads the wallpaper.
//! So it is drawn as every cut is, kept while what it is drawn from is the
//! same (\[fx0\] Ruling 9) and drawn again only when something under the part
//! changes, and its chain runs through the same memos and places. What it
//! holds of a window beneath is the window as it is drawn without effects
//! of its own: an xray holds no pane's slots (Ruling F29).
//! `tests::a_part_reads_the_xray_layers_strictly_below_its_band`,
//! `tests::a_cuts_generation_moves_only_when_it_is_redrawn`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use smithay::backend::renderer::gles::GlesTexture;
use smithay::output::Output;

use crate::effect::tree::Tree;
use crate::render::Stacked;
use crate::scripted::Layer;
use crate::stack::Band;
use crate::state::Solium;

/// Which of a monitor's layers an xray holds: a part reads the xray layers
/// strictly below its own band, so a part on `bottom` reads the background
/// alone and every part above it reads the background and bottom layers,
/// at most two cuts a monitor (Ruling F29).
/// `tests::a_part_reads_the_xray_layers_strictly_below_its_band`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Cut {
    /// The background layer.
    Background,
    /// The background and bottom layers.
    Bottom,
    /// Everything drawn strictly below one part, windows included: a live
    /// backdrop (#209), the part named by its [`owner_key`].
    Beneath(u64),
}

/// The number a live cut names its part by.
/// `tests::a_live_cut_names_its_part_and_lets_it_go_with_its_cut`.
pub(crate) fn owner_key(owner: &crate::effect::plan::Owner) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    owner.hash(&mut hasher);
    hasher.finish()
}

/// A layer `effects.xray.layers` names.
/// `tests::xray_layers_and_scale_are_read_and_checked`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum XrayLayer {
    Background,
    Bottom,
}

/// `effects.xray.straddle`: a part across two monitors reads each
/// monitor's memo (a seam at the edge), or only its own pane's monitor's
/// (amended 2026-10-08, configurable; Task 44 reads it).
/// `tests::xray_keep_straddle_and_warped_are_read_and_checked`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Straddle {
    #[default]
    Each,
    Own,
}

/// `effects.xray.warped`: a warped pane's behind T2 slot drawn beneath its
/// warp, reading the backdrop where each fragment is (`Drawn`, the default
/// since 2026-10-14), or kept in its capture at its resting rectangle
/// (`Rest`, the cheaper choice); `render::measured` reads it (Ruling F37).
/// The value is not called `live`, which `source = "live"` (X4.1) means.
/// `tests::xray_keep_straddle_and_warped_are_read_and_checked`,
/// `tests::the_default_of_effects_xray_warped_is_drawn`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Warped {
    Rest,
    #[default]
    Drawn,
}

/// `effects.xray`, held in `effect::settings::Settings.xray` (Ruling 28).
/// `tests::the_default_xray_is_the_wallpaper_alone`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct XrayConfig {
    pub(crate) layers: Vec<XrayLayer>,
    pub(crate) scale: f64,
    /// How long a cut nobody wants is kept, in ms of `state.clock`.
    pub(crate) keep_ms: u64,
    pub(crate) straddle: Straddle,
    pub(crate) warped: Warped,
}

impl Default for XrayConfig {
    fn default() -> Self {
        Self {
            layers: vec![XrayLayer::Background],
            scale: 1.0,
            keep_ms: 2000,
            straddle: Straddle::Each,
            warped: Warped::Drawn,
        }
    }
}

/// The keys `effects.xray` takes.
const KEYS: [&str; 5] = ["layers", "scale", "keep_ms", "straddle", "warped"];

/// `effects.xray`, read and checked through `Tree`'s own shape (a table's
/// named keys, a list of words): each key refused by name outside its words
/// or bounds, a number that is not finite included (`settings::tree_of`
/// keeps it), an integer scale read as a number, a whole `keep_ms` Lua holds
/// as a float (`1e4`) read, and an unknown key named with the nearest known
/// one, as Ruling 28's are.
/// `tests::xray_layers_and_scale_are_read_and_checked`,
/// `tests::xray_keep_straddle_and_warped_are_read_and_checked`,
/// `tests::effects_xray_refuses_what_is_not_its_shape`.
pub(crate) fn parse(tree: &Tree) -> Result<XrayConfig, String> {
    let Tree::Table { list, fields } = tree else {
        return Err(
            "`effects.xray` is a table: { layers = { \"background\" }, scale = 1 }".to_owned(),
        );
    };
    if !list.is_empty() {
        return Err(
            "`effects.xray` takes named keys: layers, scale, keep_ms, straddle or warped"
                .to_owned(),
        );
    }
    let mut config = XrayConfig::default();
    for (key, value) in fields {
        match key.as_str() {
            "layers" => config.layers = layers(value)?,
            "scale" => {
                let scale = match value {
                    Tree::Number(number) => *number,
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a scale of 0.25 to 1, as Lua's integer 1"
                    )]
                    Tree::Int(int) => *int as f64,
                    _ => return Err("`effects.xray.scale` is a number".to_owned()),
                };
                if !(0.25..=1.0).contains(&scale) {
                    return Err(format!("`effects.xray.scale` is 0.25 to 1, not {scale}"));
                }
                config.scale = scale;
            }
            "keep_ms" => {
                config.keep_ms = match value {
                    Tree::Int(ms) => u64::try_from(*ms).ok().filter(|ms| *ms <= 60_000),
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "whole and within 0 to 60000, checked first, so exact"
                    )]
                    Tree::Number(ms) if ms.fract() == 0.0 && (0.0..=60_000.0).contains(ms) => {
                        Some(*ms as u64)
                    }
                    _ => None,
                }
                .ok_or_else(|| {
                    "`effects.xray.keep_ms` is a whole number of ms, 0 to 60000".to_owned()
                })?;
            }
            "straddle" => {
                config.straddle = match value {
                    Tree::Text(word) if word == "each" => Straddle::Each,
                    Tree::Text(word) if word == "own" => Straddle::Own,
                    _ => return Err("`effects.xray.straddle` is each or own".to_owned()),
                };
            }
            "warped" => {
                config.warped = match value {
                    Tree::Text(word) if word == "rest" => Warped::Rest,
                    Tree::Text(word) if word == "drawn" => Warped::Drawn,
                    _ => return Err("`effects.xray.warped` is rest or drawn".to_owned()),
                };
            }
            other => {
                let meant = solium_effects::spec::nearest(other, &KEYS)
                    .map(|meant| format!("; did you mean `{meant}`?"))
                    .unwrap_or_default();
                return Err(format!(
                    "`{other}` is not an xray key: layers, scale, keep_ms, straddle or warped{meant}"
                ));
            }
        }
    }
    Ok(config)
}

/// `effects.xray.layers`: a list of words, nothing named beside them, so a
/// table of names is refused rather than read as no layers; and
/// `"background"` among them, since every cut holds the wallpaper: no
/// layers would turn every T2 effect off unsaid (a rule's `effect = false`
/// says it), and `"bottom"` alone would hand a window the wallpaper anyway
/// and a part on `bottom` nothing.
/// `tests::effects_xray_refuses_what_is_not_its_shape`.
fn layers(value: &Tree) -> Result<Vec<XrayLayer>, String> {
    let list = match value {
        Tree::Table { list, fields } if fields.is_empty() => list,
        _ => return Err("`effects.xray.layers` is a list of words".to_owned()),
    };
    let layers: Vec<XrayLayer> = list
        .iter()
        .map(|word| match word {
            Tree::Text(word) if word == "background" => Ok(XrayLayer::Background),
            Tree::Text(word) if word == "bottom" => Ok(XrayLayer::Bottom),
            Tree::Text(other) => Err(format!(
                "`{other}` is not an xray layer: background or bottom"
            )),
            _ => Err("`effects.xray.layers` is a list of words".to_owned()),
        })
        .collect::<Result<_, _>>()?;
    if !layers.contains(&XrayLayer::Background) {
        return Err(
            "`effects.xray.layers` holds \"background\", alone or with \"bottom\"".to_owned(),
        );
    }
    Ok(layers)
}

impl XrayConfig {
    /// The cut a part on `band` reads: the configured layers strictly below
    /// it, or none. `tests::a_part_reads_the_xray_layers_strictly_below_its_band`.
    pub(crate) fn cut_below(&self, band: Band) -> Option<Cut> {
        let has = |layer| self.layers.contains(&layer);
        match band {
            Band::Layer(Layer::Background, _) => None,
            Band::Layer(Layer::Bottom, _) => has(XrayLayer::Background).then_some(Cut::Background),
            _ if has(XrayLayer::Bottom) => Some(Cut::Bottom),
            _ => has(XrayLayer::Background).then_some(Cut::Background),
        }
    }
}

/// Where an xray's elements come from. The session's is [`SessionLayers`];
/// FX6's lock xray is another.
/// `state::tests::real_client::reflow_on_close::stacking::xray_holds_the_cuts_bands_and_nothing_else`.
pub(crate) trait XraySource: std::fmt::Debug {
    fn entries(&self, state: &Solium, output: &Output, cut: Cut) -> Vec<Stacked>;
}

/// The session's layers in the cut's bands, in `render::stacked`'s order.
/// `state::tests::real_client::reflow_on_close::stacking::xray_holds_the_cuts_bands_and_nothing_else`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SessionLayers;

impl XraySource for SessionLayers {
    fn entries(&self, state: &Solium, output: &Output, cut: Cut) -> Vec<Stacked> {
        let Some(screen) = state.space.output_geometry(output) else {
            return Vec::new();
        };
        if let Cut::Beneath(key) = cut {
            return state
                .xrays
                .owner_beneath(key)
                .map_or_else(Vec::new, |owner| {
                    beneath(
                        crate::render::stacked(state, screen, state.clock.now()),
                        owner,
                        &output.name(),
                    )
                });
        }
        crate::render::stacked(state, screen, state.clock.now())
            .into_iter()
            .filter(|each| match crate::render::band_of(state, each) {
                Band::Layer(Layer::Background, _) => true,
                Band::Layer(Layer::Bottom, _) => cut == Cut::Bottom,
                _ => false,
            })
            .collect()
    }
}

/// What is drawn strictly below `owner`, from a monitor's entries topmost
/// first: every entry after its own, and in its own group of panes the panes
/// after it. Nothing when it is not on that monitor.
/// `state::tests::real_client::reflow_on_close::stacking::a_live_cut_holds_what_is_below_its_part_and_nothing_above`.
pub(crate) fn beneath(
    entries: Vec<Stacked>,
    owner: &crate::effect::plan::Owner,
    monitor: &str,
) -> Vec<Stacked> {
    use crate::effect::plan::Owner as Whose;
    use smithay::reexports::wayland_server::Resource as _;
    let mut below = Vec::new();
    let mut found = false;
    for each in entries {
        if found {
            below.push(each);
            continue;
        }
        match (each, owner) {
            (Stacked::Panes(nodes), Whose::Pane(pane, _)) => {
                if let Some(at) = nodes.iter().position(|((id, ..), _)| id == pane) {
                    found = true;
                    let rest: Vec<_> = nodes.into_iter().skip(at + 1).collect();
                    if !rest.is_empty() {
                        below.push(Stacked::Panes(rest));
                    }
                }
            }
            (Stacked::Layer(surface, _), Whose::LayerShell(object)) => {
                found = surface.wl_surface().id() == *object;
            }
            (Stacked::Surface(id, ..), Whose::Surface(own, on)) => {
                found = id == *own && on == monitor;
            }
            _ => {}
        }
    }
    below
}

/// Every monitor's xray captures, their generations, what is wanted this
/// pass, when each was last wanted, and which cuts have been built at
/// least once (the warm-up's half, spec C14).
///
/// A generation is the one counter's next value, whatever the monitor or
/// the cut, and [`Xrays::sweep`] never resets the counter: so a cut given
/// back and built again, perhaps over a new wallpaper, never carries a
/// generation a memo kept by `effects.memo.keep_passes` ran over, or a
/// T2 key holds, and no two cuts share one, so a slot whose cut changes
/// runs again too.
/// `tests::a_cuts_generation_moves_only_when_it_is_redrawn`,
/// `tests::a_memo_kept_past_its_cuts_sweep_runs_again_on_the_rebuilt_cut`,
/// `tests::no_two_cuts_or_monitors_ever_share_a_generation`.
#[derive(Debug, Default)]
pub(crate) struct Xrays {
    captures: HashMap<(String, Cut), crate::keyed::Capture>,
    generations: HashMap<(String, Cut), u64>,
    /// The last generation handed out, to any cut on any monitor.
    last_generation: u64,
    wanted: HashSet<(String, Cut)>,
    last_wanted: HashMap<(String, Cut), Duration>,
    built: HashSet<(String, Cut)>,
    asked: HashSet<(String, Cut)>,
    /// Which part each live cut is beneath, by [`owner_key`].
    beneath: HashMap<u64, crate::effect::plan::Owner>,
}

impl Xrays {
    /// A slot reads `(output, cut)` this pass.
    /// `tests::a_cuts_generation_moves_only_when_it_is_redrawn`.
    pub(crate) fn want(&mut self, output: &str, cut: Cut) {
        self.wanted.insert((output.to_owned(), cut));
        self.asked.insert((output.to_owned(), cut));
    }

    /// A live cut's part, named as it is wanted, so its entries can be found
    /// (`SessionLayers`).
    /// `tests::a_live_cut_names_its_part_and_lets_it_go_with_its_cut`.
    pub(crate) fn name_beneath(&mut self, cut: Cut, owner: &crate::effect::plan::Owner) {
        if let Cut::Beneath(key) = cut {
            self.beneath.entry(key).or_insert_with(|| owner.clone());
        }
    }

    /// The part a live cut is beneath.
    pub(crate) fn owner_beneath(&self, key: u64) -> Option<&crate::effect::plan::Owner> {
        self.beneath.get(&key)
    }

    /// Whether a live cut is wanted on `output` this pass: there nothing
    /// beneath a part may be scanned out on a plane of its own, which the
    /// cut could not hold (`tty.rs`).
    /// `tests::a_live_cut_names_its_part_and_lets_it_go_with_its_cut`.
    pub(crate) fn live_on(&self, output: &str) -> bool {
        self.wanted
            .iter()
            .any(|(monitor, cut)| monitor == output && matches!(cut, Cut::Beneath(_)))
    }

    /// The cuts wanted this pass, in order.
    /// `tests::a_cuts_generation_moves_only_when_it_is_redrawn`.
    pub(crate) fn wanted(&self) -> Vec<(String, Cut)> {
        let mut wanted: Vec<_> = self.wanted.iter().cloned().collect();
        wanted.sort();
        wanted
    }

    /// `tests::a_cut_nobody_wants_is_kept_for_keep_ms`.
    pub(crate) fn clear_wants(&mut self) {
        self.wanted.clear();
    }

    /// The capture `(output, cut)` is drawn into: `offscreen::draw` and
    /// `offscreen::kept` hold an xray's target here.
    /// `offscreen::tests::an_xrays_capture_is_held_in_xrays`.
    pub(crate) fn capture_mut(&mut self, output: &str, cut: Cut) -> &mut crate::keyed::Capture {
        self.captures.entry((output.to_owned(), cut)).or_default()
    }

    /// This pass's capture of `(output, cut)` is in hand: drawn anew, or
    /// kept. Only a redraw moves the generation memos key on, to the one
    /// counter's next value.
    /// `tests::a_cuts_generation_moves_only_when_it_is_redrawn`,
    /// `tests::no_two_cuts_or_monitors_ever_share_a_generation`.
    pub(crate) fn drawn(&mut self, output: &str, cut: Cut, redrawn: bool) {
        let key = (output.to_owned(), cut);
        self.built.insert(key.clone());
        if redrawn {
            self.last_generation = self.last_generation.wrapping_add(1);
            self.generations.insert(key, self.last_generation);
        }
    }

    /// `tests::a_cuts_generation_moves_only_when_it_is_redrawn`.
    pub(crate) fn generation(&self, output: &str, cut: Cut) -> Option<u64> {
        self.generations.get(&(output.to_owned(), cut)).copied()
    }

    /// The cut's picture, while it is held: what every T2 run binds as
    /// `backdrop` (`render::run_xray`, `render::run_slots`).
    pub(crate) fn texture(&self, output: &str, cut: Cut) -> Option<&GlesTexture> {
        self.captures
            .get(&(output.to_owned(), cut))?
            .target()
            .map(|target| target.texture())
    }

    /// Every cut ever wanted has been built once: the first frame has its
    /// xray (spec C14), as `Solium::effects_ready` reads it.
    /// `tests::a_cuts_generation_moves_only_when_it_is_redrawn`.
    pub(crate) fn ready(&self) -> bool {
        self.asked.iter().all(|key| self.built.contains(key))
    }

    /// A cut not wanted for `idle` (`effects.xray.keep_ms`) gives its target
    /// back, with its generation and its built mark; one wanted again before
    /// then is the capture it was, so its memos need no run (Ruling F29).
    /// The generation counter is never reset (the type's doc says why).
    /// `tests::a_cut_nobody_wants_is_kept_for_keep_ms`,
    /// `tests::a_memo_kept_past_its_cuts_sweep_runs_again_on_the_rebuilt_cut`.
    pub(crate) fn sweep(&mut self, now: Duration, idle: Duration, pool: &mut crate::pool::Pool) {
        for key in &self.wanted {
            self.last_wanted.insert(key.clone(), now);
        }
        let last_wanted = &self.last_wanted;
        let stale = |key: &(String, Cut)| {
            last_wanted
                .get(key)
                .is_none_or(|at| now.saturating_sub(*at) > idle)
        };
        self.captures.retain(|key, capture| {
            let keep = !stale(key);
            if !keep {
                capture.release(pool);
            }
            keep
        });
        self.generations.retain(|key, _| !stale(key));
        self.built.retain(|key| !stale(key));
        self.asked.retain(|key| !stale(key));
        let held: HashSet<u64> = self
            .captures
            .keys()
            .chain(self.wanted.iter())
            .filter_map(|(_, cut)| match cut {
                Cut::Beneath(key) => Some(*key),
                _ => None,
            })
            .collect();
        self.beneath.retain(|key, _| held.contains(key));
        self.last_wanted
            .retain(|_, at| now.saturating_sub(*at) <= idle);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Cut, XrayConfig, XrayLayer, Xrays, parse};
    use crate::effect::memo::{MemoKey, Memos};
    use crate::pool::Pool;
    use crate::scripted::Layer;
    use crate::stack::{Band, Owner};

    fn both() -> XrayConfig {
        XrayConfig {
            layers: vec![XrayLayer::Background, XrayLayer::Bottom],
            ..XrayConfig::default()
        }
    }

    /// **The default is the wallpaper alone** ([16] decision 2), a cut
    /// kept 2 s, one result per monitor, read where a warped pane is drawn.
    #[test]
    fn the_default_xray_is_the_wallpaper_alone() {
        assert_eq!(
            XrayConfig::default(),
            XrayConfig {
                layers: vec![XrayLayer::Background],
                scale: 1.0,
                keep_ms: 2000,
                straddle: super::Straddle::Each,
                warped: super::Warped::Drawn
            }
        );
    }

    /// **A warped pane reads the backdrop where it is drawn, by default**
    /// (Ruling F37, amended 2026-10-14): `"rest"` is the cheaper choice,
    /// read and kept.
    #[test]
    fn the_default_of_effects_xray_warped_is_drawn() {
        assert_eq!(XrayConfig::default().warped, super::Warped::Drawn);
        assert_eq!(
            parse(&tree("{}")).expect("read").warped,
            super::Warped::Drawn
        );
        assert_eq!(
            parse(&tree(r#"{ warped = "rest" }"#)).expect("read").warped,
            super::Warped::Rest
        );
    }

    /// **A part reads the xray layers strictly below its own band**: a
    /// widget on `bottom` reads the wallpaper only, never itself; a window
    /// reads both; a part on `background` reads nothing.
    #[test]
    fn a_part_reads_the_xray_layers_strictly_below_its_band() {
        let config = both();
        assert_eq!(
            config.cut_below(Band::Layer(Layer::Bottom, Owner::Client)),
            Some(Cut::Background)
        );
        assert_eq!(config.cut_below(Band::Windows), Some(Cut::Bottom));
        assert_eq!(
            config.cut_below(Band::Layer(Layer::Top, Owner::Script)),
            Some(Cut::Bottom)
        );
        assert_eq!(
            config.cut_below(Band::Layer(Layer::Background, Owner::Script)),
            None
        );
        assert_eq!(
            XrayConfig::default().cut_below(Band::Windows),
            Some(Cut::Background)
        );
    }

    fn tree(lua: &str) -> crate::effect::tree::Tree {
        crate::effect::tree::tests::tree(lua).expect("a table")
    }

    /// **`layers` and `scale` are read and checked by name**; the shipped
    /// `scale = 1` is a Lua integer (`Tree::Int`) and reads as 1.
    #[test]
    fn xray_layers_and_scale_are_read_and_checked() {
        assert_eq!(
            parse(&tree(
                r#"{ layers = { "background", "bottom" }, scale = 0.5 }"#
            )),
            Ok(XrayConfig {
                scale: 0.5,
                ..both()
            })
        );
        assert_eq!(
            parse(&tree(r#"{ layers = { "background" }, scale = 1 }"#)),
            Ok(XrayConfig::default())
        );
        assert!(
            parse(&tree(r#"{ layers = { "top" } }"#))
                .expect_err("refused")
                .contains("background or bottom")
        );
        assert!(
            parse(&tree(r#"{ scale = 0.1 }"#))
                .expect_err("refused")
                .contains("0.25")
        );
        assert!(
            parse(&tree(r#"{ sale = 1 }"#))
                .expect_err("refused")
                .contains("layers, scale, keep_ms, straddle or warped")
        );
    }

    /// **`keep_ms`, `straddle` and `warped` are read and checked**
    /// (amended 2026-10-08, configurable).
    #[test]
    fn xray_keep_straddle_and_warped_are_read_and_checked() {
        let read = parse(&tree(
            r#"{ keep_ms = 5000, straddle = "own", warped = "drawn" }"#,
        ))
        .expect("read");
        assert_eq!(
            (read.keep_ms, read.straddle, read.warped),
            (5000, super::Straddle::Own, super::Warped::Drawn)
        );
        assert!(
            parse(&tree("{ keep_ms = 70000 }"))
                .expect_err("refused")
                .contains("0 to 60000")
        );
        assert!(
            parse(&tree(r#"{ straddle = "both" }"#))
                .expect_err("refused")
                .contains("each or own")
        );
        assert!(
            parse(&tree(r#"{ warped = "tilted" }"#))
                .expect_err("refused")
                .contains("rest or drawn")
        );
    }

    /// **`effects.xray` is a table of named keys in their shapes**: a
    /// whole `keep_ms` Lua holds as a float (`1e4`) is read; anything else
    /// is refused naming the key, a number that is not finite included (as
    /// `settings::tree_of` keeps it), a `layers` table with names in it
    /// rather than read as no layers, a `layers` without `"background"`
    /// (none at all would turn every T2 effect off unsaid, and `bottom`
    /// alone would hand windows the wallpaper anyway and a part on `bottom`
    /// nothing), and a misspelt key with the nearest.
    #[test]
    fn effects_xray_refuses_what_is_not_its_shape() {
        use crate::effect::tree::Tree;
        assert_eq!(
            parse(&tree("{ keep_ms = 1e4 }")).map(|config| config.keep_ms),
            Ok(10_000)
        );
        let not_finite = |key: &str, number: f64| Tree::Table {
            list: Vec::new(),
            fields: [(key.to_owned(), Tree::Number(number))].into(),
        };
        for (tree, says) in [
            (
                not_finite("scale", f64::NAN),
                "`effects.xray.scale` is 0.25 to 1",
            ),
            (
                not_finite("scale", f64::INFINITY),
                "`effects.xray.scale` is 0.25 to 1",
            ),
            (
                not_finite("keep_ms", f64::INFINITY),
                "`effects.xray.keep_ms` is a whole number",
            ),
        ] {
            let refused = parse(&tree).expect_err(says);
            assert!(refused.contains(says), "{says}: {refused}");
        }
        for (lua, says) in [
            ("true", "`effects.xray` is a table"),
            (r#"{ "background" }"#, "takes named keys"),
            (
                "{ layers = { background = true } }",
                "`effects.xray.layers` is a list of words",
            ),
            (
                "{ layers = 'background' }",
                "`effects.xray.layers` is a list of words",
            ),
            (
                "{ layers = { 1 } }",
                "`effects.xray.layers` is a list of words",
            ),
            (
                "{ layers = {} }",
                "`effects.xray.layers` holds \"background\"",
            ),
            (
                "{ layers = { 'bottom' } }",
                "`effects.xray.layers` holds \"background\"",
            ),
            ("{ scale = 'half' }", "`effects.xray.scale` is a number"),
            (
                "{ keep_ms = 1.5 }",
                "`effects.xray.keep_ms` is a whole number",
            ),
            (
                "{ keep_ms = -1 }",
                "`effects.xray.keep_ms` is a whole number",
            ),
            ("{ straddle = 1 }", "`effects.xray.straddle` is each or own"),
            ("{ sale = 1 }", "did you mean `scale`?"),
        ] {
            let refused = parse(&tree(lua)).expect_err(lua);
            assert!(refused.contains(says), "{lua}: {refused}");
        }
    }

    /// **A cut wanted is built once before it is ready, and a redraw moves
    /// its generation; a kept one does not.**
    #[test]
    fn a_cuts_generation_moves_only_when_it_is_redrawn() {
        let mut xrays = Xrays::default();
        assert!(xrays.ready(), "nothing wanted is ready");
        xrays.want("DP-1", Cut::Background);
        assert_eq!(xrays.wanted(), vec![("DP-1".to_owned(), Cut::Background)]);
        assert!(!xrays.ready());
        xrays.drawn("DP-1", Cut::Background, true);
        let first = xrays.generation("DP-1", Cut::Background);
        assert!(xrays.ready());
        xrays.drawn("DP-1", Cut::Background, false);
        assert_eq!(xrays.generation("DP-1", Cut::Background), first);
        xrays.drawn("DP-1", Cut::Background, true);
        assert_ne!(xrays.generation("DP-1", Cut::Background), first);
    }

    /// **A cut nobody wants is kept for `effects.xray.keep_ms`** (Ruling
    /// F29; amended 2026-10-08, configurable: the first draft named "the
    /// pool's idle limit", which `pool.rs` does not have), so an `auto` bar
    /// uncovered after a cover reads the capture it had, with its
    /// generation, and its memos do not run again (*Review log* finding 15).
    #[test]
    fn a_cut_nobody_wants_is_kept_for_keep_ms() {
        let (mut xrays, mut pool) = (Xrays::default(), Pool::new(0));
        let idle = Duration::from_millis(XrayConfig::default().keep_ms);
        xrays.want("DP-1", Cut::Background);
        xrays.drawn("DP-1", Cut::Background, true);
        let first = xrays.generation("DP-1", Cut::Background);
        xrays.sweep(Duration::ZERO, idle, &mut pool);
        xrays.clear_wants();
        assert!(xrays.wanted().is_empty());
        xrays.sweep(Duration::from_millis(100), idle, &mut pool);
        xrays.want("DP-1", Cut::Background);
        assert_eq!(
            xrays.generation("DP-1", Cut::Background),
            first,
            "uncovered, rebuilt"
        );
        assert!(xrays.ready());
        xrays.clear_wants();
        xrays.sweep(Duration::from_secs(3), idle, &mut pool);
        assert_eq!(
            xrays.generation("DP-1", Cut::Background),
            None,
            "kept past keep_ms"
        );
        // Each keeps a cut wanted in the very pass it sweeps, keep_ms = 0
        // too, or its target would be given back and rebuilt every pass and
        // every memo over it run again; then keep_ms = 0 gives it back at
        // the next sweep, and keep_ms = 5000 still holds it at 3 s.
        for (keep_ms, held_at_3s) in [(0, false), (5000, true)] {
            let (mut xrays, mut pool) = (Xrays::default(), Pool::new(0));
            let idle = Duration::from_millis(keep_ms);
            xrays.want("DP-1", Cut::Background);
            xrays.drawn("DP-1", Cut::Background, true);
            xrays.sweep(Duration::ZERO, idle, &mut pool);
            assert!(
                xrays.generation("DP-1", Cut::Background).is_some(),
                "keep_ms = {keep_ms}: swept in the pass that wants it"
            );
            xrays.clear_wants();
            xrays.sweep(
                Duration::from_millis(if keep_ms == 0 { 1 } else { 3000 }),
                idle,
                &mut pool,
            );
            assert_eq!(
                xrays.generation("DP-1", Cut::Background).is_some(),
                held_at_3s,
                "keep_ms = {keep_ms}"
            );
        }
    }

    /// **A memo kept past its cut's sweep runs again on the rebuilt cut**:
    /// `effects.memo.keep_passes` may keep a memo longer than
    /// `effects.xray.keep_ms` keeps the cut it ran over. Swept and built
    /// again, perhaps over a new wallpaper, the cut's generation is new, so
    /// the memo runs again rather than drawing the old wallpaper's blur.
    #[test]
    fn a_memo_kept_past_its_cuts_sweep_runs_again_on_the_rebuilt_cut() {
        let (mut xrays, mut memos, mut pool) = (Xrays::default(), Memos::default(), Pool::new(0));
        let idle = Duration::from_millis(XrayConfig::default().keep_ms);
        let key = MemoKey {
            monitor: "DP-1".to_owned(),
            cut: Cut::Background,
            upto: 7,
        };
        xrays.want("DP-1", Cut::Background);
        xrays.drawn("DP-1", Cut::Background, true);
        let old = xrays.generation("DP-1", Cut::Background).expect("built");
        memos.want(&key, 1);
        memos.ran(&key, old);
        xrays.sweep(Duration::ZERO, idle, &mut pool);
        xrays.clear_wants();
        // Nobody reads either for 3 s, 700 passes: the cut is given back
        // at keep_ms, the memo kept by keep_passes = 3600.
        memos.sweep(700, 3600, &mut pool);
        xrays.sweep(Duration::from_secs(3), idle, &mut pool);
        assert_eq!(
            (xrays.generation("DP-1", Cut::Background), memos.len()),
            (None, 1),
            "the premise: the cut was given back and the memo kept"
        );
        xrays.want("DP-1", Cut::Background);
        xrays.drawn("DP-1", Cut::Background, true);
        let rebuilt = xrays.generation("DP-1", Cut::Background).expect("rebuilt");
        assert!(
            memos.needs_run(&key, rebuilt),
            "the old wallpaper's memo would be drawn over the rebuilt cut"
        );
    }

    /// **No two cuts or monitors ever share a generation**: a T2 key holds
    /// its place's cut generation (`memo::T2Key.xray`), so a slot whose cut
    /// changes, or a place on another monitor, never finds the generation
    /// it ran over.
    #[test]
    fn no_two_cuts_or_monitors_ever_share_a_generation() {
        let mut xrays = Xrays::default();
        let mut seen = Vec::new();
        for (output, cut) in [
            ("DP-1", Cut::Background),
            ("DP-1", Cut::Bottom),
            ("DP-2", Cut::Background),
            ("DP-1", Cut::Background),
        ] {
            xrays.want(output, cut);
            xrays.drawn(output, cut, true);
            seen.push(xrays.generation(output, cut).expect("drawn"));
        }
        assert!(
            seen.windows(2).all(|pair| pair[0] < pair[1]),
            "each redraw a new, larger generation: {seen:?}"
        );
    }

    /// **A live cut names its part, says it is live where it is wanted, and
    /// lets its part go with its cut** (#209): a cut nobody wants for
    /// `keep_ms` takes its part's name with it.
    #[test]
    fn a_live_cut_names_its_part_and_lets_it_go_with_its_cut() {
        use crate::effect::plan::{Owner, PaneSlot};
        let owner = Owner::Pane(crate::pane::PaneId::from_raw(7), PaneSlot::Client);
        let key = super::owner_key(&owner);
        assert_eq!(key, super::owner_key(&owner.clone()), "one part, one name");
        let cut = Cut::Beneath(key);
        let mut xrays = Xrays::default();
        xrays.name_beneath(cut, &owner);
        xrays.want("DP-1", cut);
        assert_eq!(xrays.owner_beneath(key), Some(&owner));
        assert!(xrays.live_on("DP-1") && !xrays.live_on("DP-2"));
        let mut pool = Pool::new(0);
        xrays.sweep(
            Duration::from_millis(0),
            Duration::from_millis(100),
            &mut pool,
        );
        xrays.clear_wants();
        assert!(!xrays.live_on("DP-1"), "not wanted this pass");
        assert_eq!(
            xrays.owner_beneath(key),
            Some(&owner),
            "kept while its cut is"
        );
        xrays.sweep(
            Duration::from_millis(500),
            Duration::from_millis(100),
            &mut pool,
        );
        assert_eq!(xrays.owner_beneath(key), None, "gone with its cut");
    }
}
