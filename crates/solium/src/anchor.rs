//! Names on screen that a window can be flown to or from, and where each is
//! this pass.
//!
//! A hosted scene publishes a region (`Solium.region`, read after polish by
//! the QML host, `Solium::read_regions`); the compositor and the scripts aim
//! at its name, never at a snapshot of where it was, so a window flying into
//! a dock icon follows the icon while the dock slides. Pure: no session, no
//! Qt, no GPU, so every rule here is a unit test.
//! `tests::a_name_resolves_on_the_flights_own_monitor_first`.
//!
//! A name is `<surface>/<path>`: the `sol.surface` the scene belongs to, then
//! the path its item wrote, so the shell's `dock/icons/foot` is
//! `shell/dock/icons/foot`. The shell runs once per monitor, so one name is a
//! region on each, and which monitor's copy a flight means is
//! `anchors.same` ([`Settings`]).
#![cfg_attr(not(test), expect(dead_code, reason = "Task A2 fills the registry"))]

use std::collections::HashMap;

use smithay::utils::{Logical, Point, Rectangle};

use crate::scripted::SurfaceId;

/// A rectangle in logical pixels: a region's place in its scene, or on the
/// desktop once resolved.
pub(crate) type Rect = Rectangle<f64, Logical>;

/// A name, interned, so the registry is keyed by a number.
/// `tests::a_gone_change_removes_one_region`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Key(u32);

/// Every name the registry has seen, both ways. Names are never forgotten:
/// a region that comes back keeps its key.
#[derive(Debug, Default)]
pub(crate) struct Names {
    keys: HashMap<String, Key>,
    names: Vec<String>,
}

impl Names {
    /// The key for `name`, made on first sight.
    pub(crate) fn intern(&mut self, name: &str) -> Key {
        if let Some(key) = self.keys.get(name) {
            return *key;
        }
        let key = Key(u32::try_from(self.names.len()).unwrap_or(u32::MAX));
        self.names.push(name.to_owned());
        self.keys.insert(name.to_owned(), key);
        key
    }

    /// The key of a name seen before, if it was.
    pub(crate) fn find(&self, name: &str) -> Option<Key> {
        self.keys.get(name).copied()
    }

    /// The name a key stands for; empty for a key this table never gave.
    pub(crate) fn name(&self, key: Key) -> &str {
        usize::try_from(key.0)
            .ok()
            .and_then(|index| self.names.get(index))
            .map_or("", String::as_str)
    }
}

/// Whose regions these are: one instance of a `sol.surface`, on one monitor.
/// `tests::a_scene_torn_down_takes_its_regions_with_it`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Owner {
    pub(crate) surface: SurfaceId,
    pub(crate) output: String,
}

/// One published region as the registry keeps it: where it is in its scene
/// now (`local`), where it was in the scene's last render (`rendered`), its
/// corner radii (top-left, top-right, bottom-right, bottom-left), whether its
/// pixels could be taken (`shown`), which render `rendered` is from, and the
/// registry's generation when any of it last changed.
/// `tests::generations_rise_on_every_change`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Published {
    pub(crate) local: Rect,
    pub(crate) rendered: Rect,
    pub(crate) radius: [f32; 4],
    pub(crate) shown: bool,
    pub(crate) rendered_pass: u64,
    pub(crate) generation: u64,
}

/// What one region did since its scene was last asked: it is now as given,
/// or it is `gone`. `path` is the item's own `Solium.region`, without the
/// surface's name.
/// `tests::a_gone_change_removes_one_region`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RegionChange {
    pub(crate) path: String,
    pub(crate) gone: bool,
    pub(crate) local: Rect,
    pub(crate) rendered: Rect,
    pub(crate) radius: [f32; 4],
    pub(crate) shown: bool,
    pub(crate) rendered_pass: u64,
}

/// Every region the hosted scenes publish, by name and monitor.
/// `tests::a_name_resolves_on_the_flights_own_monitor_first`.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    entries: HashMap<(Key, String), (Owner, Published)>,
    /// The last generation given out: every change takes the next one, so a
    /// region that goes and comes back never repeats one.
    generation: u64,
}

impl Registry {
    /// Take what one owner's scene reported. A region whose path is outside
    /// the alphabet names allow is skipped with a warning naming the surface
    /// and the path (Ruling M2); a change that changes nothing keeps the
    /// region's generation.
    /// `tests::generations_rise_on_every_change`,
    /// `tests::a_path_out_of_its_alphabet_is_skipped_by_the_registry`.
    pub(crate) fn apply(
        &mut self,
        names: &mut Names,
        owner: &Owner,
        surface: &str,
        changes: &[RegionChange],
    ) {
        for change in changes {
            if let Err(why) = check_path(&change.path) {
                tracing::warn!(surface, path = change.path, "a region was skipped: {why}");
                continue;
            }
            let key = names.intern(&format!("{surface}/{}", change.path));
            let slot = (key, owner.output.clone());
            if change.gone {
                if self
                    .entries
                    .get(&slot)
                    .is_some_and(|(held, _)| held == owner)
                {
                    self.entries.remove(&slot);
                }
                continue;
            }
            let mut now = Published {
                local: change.local,
                rendered: change.rendered,
                radius: change.radius,
                shown: change.shown,
                rendered_pass: change.rendered_pass,
                generation: 0,
            };
            if let Some((held, before)) = self.entries.get(&slot)
                && held == owner
                && (Published {
                    generation: 0,
                    ..*before
                }) == now
            {
                continue;
            }
            self.generation += 1;
            now.generation = self.generation;
            self.entries.insert(slot, (owner.clone(), now));
        }
    }

    /// Forget every region of one owner: its scene was torn down.
    /// `tests::a_scene_torn_down_takes_its_regions_with_it`.
    pub(crate) fn drop_owner(&mut self, owner: &Owner) {
        self.entries.retain(|_, (held, _)| held != owner);
    }

    /// Keep only the owners `live` answers for: what the compositor does once
    /// a pass, so an instance dropped anywhere (its monitor gone, its surface
    /// removed or rebuilt) takes its regions with it.
    /// `state::tests::a_torn_down_scene_takes_its_regions_with_it`.
    pub(crate) fn retain_owners(&mut self, mut live: impl FnMut(&Owner) -> bool) {
        self.entries.retain(|_, (held, _)| live(held));
    }

    /// Whether nothing is published.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The region a name has on one monitor, with its owner.
    pub(crate) fn on(&self, key: Key, output: &str) -> Option<(&Owner, &Published)> {
        self.entries
            .get(&(key, output.to_owned()))
            .map(|(owner, published)| (owner, published))
    }

    /// Every region whose name starts with `prefix`, as `(name, owner,
    /// published)`, by name and then monitor: what `sol.anchors` lists.
    /// `tests::under_lists_a_prefix_by_name_and_monitor`.
    pub(crate) fn under<'a>(
        &'a self,
        names: &'a Names,
        prefix: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a Owner, &'a Published)> + 'a {
        let mut found: Vec<(&'a str, &'a Owner, &'a Published)> = self
            .entries
            .iter()
            .map(|((key, _), (owner, published))| (names.name(*key), owner, published))
            .filter(|(name, _, _)| name.starts_with(prefix))
            .collect();
        found.sort_by(|a, b| (a.0, &a.1.output).cmp(&(b.0, &b.1.output)));
        found.into_iter()
    }
}

/// One region as `sol.anchors` lists it: its name, the monitor its copy is
/// on, where it is on the desktop, its corners and whether it is shown.
/// `script::tests::sol_anchors_lists_the_regions_under_a_prefix`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub(crate) name: String,
    pub(crate) monitor: String,
    pub(crate) rect: Rect,
    pub(crate) radius: [f32; 4],
    pub(crate) shown: bool,
}

/// One link of a chain, checked: a name (a region, `pane/<id>` or
/// `monitor/…`), or today's aims, a window by its script id, a surface by its
/// `sol.surface` name, or a rectangle.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "chains are read by the launch from an anchor (Task A5)"
    )
)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Link {
    Name(String),
    Window(u64),
    Surface(String),
    Rect(Rect),
}

/// Where a flight goes, link by link: the first that resolves wins.
/// `tests::a_chain_takes_its_first_link_that_resolves`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "chains are read by the launch from an anchor (Task A5)"
    )
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Chain(pub(crate) Vec<Link>);

/// A chain as a script or an action wrote it, before it is checked: a name,
/// a list, `{ name, otherwise = … }`, or one of today's aims.
/// `tests::nested_otherwise_reads_as_one_list`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "chains are read by the launch from an anchor (Task A5)"
    )
)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Given {
    Text(String),
    List(Vec<Given>),
    Otherwise(Box<Given>, Box<Given>),
    Window(u64),
    Surface(String),
    Rect(Rect),
}

/// Read a chain as written into links: a list in order, nested lists and
/// `otherwise`s flattened. A name in a reserved namespace is refused naming
/// what it is kept for, and one with a segment outside the alphabet quoting
/// the segment.
/// `tests::nested_otherwise_reads_as_one_list`,
/// `tests::a_reserved_namespace_is_refused_by_name`,
/// `tests::a_path_out_of_its_alphabet_is_refused`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "chains are read by the launch from an anchor (Task A5)"
    )
)]
pub(crate) fn parse_chain(given: &Given) -> Result<Chain, String> {
    fn walk(given: &Given, into: &mut Vec<Link>) -> Result<(), String> {
        match given {
            Given::Text(name) => {
                check_name(name)?;
                into.push(Link::Name(name.clone()));
            }
            Given::List(items) => {
                for item in items {
                    walk(item, into)?;
                }
            }
            Given::Otherwise(first, rest) => {
                walk(first, into)?;
                walk(rest, into)?;
            }
            Given::Window(id) => into.push(Link::Window(*id)),
            Given::Surface(name) => into.push(Link::Surface(name.clone())),
            Given::Rect(rect) => into.push(Link::Rect(*rect)),
        }
        Ok(())
    }
    let mut links = Vec::new();
    walk(given, &mut links)?;
    Ok(Chain(links))
}

/// The namespaces kept for regions that are not a hosted scene's, and what
/// each is kept for (Ruling M2).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "chains are read by the launch from an anchor (Task A5)"
    )
)]
const RESERVED: [(&str, &str); 4] = [
    ("lock/", "the lock screen's regions"),
    ("layer/", "layer-shell surfaces (#50)"),
    ("toplevel/", "client windows (#50)"),
    ("lua/", "places a script names with sol.anchor"),
];

/// Whether one segment is in the alphabet names are written in.
fn in_alphabet(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '~' | '-'))
}

/// A region's path: segments of `[A-Za-z0-9._%~-]+` joined by `/`.
/// `tests::a_path_out_of_its_alphabet_is_skipped_by_the_registry`.
fn check_path(path: &str) -> Result<(), String> {
    match path.split('/').find(|segment| !in_alphabet(segment)) {
        Some(segment) => Err(format!(
            "the segment \"{segment}\" of \"{path}\" is not letters, digits and . _ % ~ -"
        )),
        None => Ok(()),
    }
}

/// A name in a chain: a path whose segments may also hold the templates
/// `{id}` and `{app_id}`, or a link that is exactly `{launch}` or `{from}`,
/// outside the reserved namespaces.
/// `tests::a_reserved_namespace_is_refused_by_name`,
/// `tests::a_path_out_of_its_alphabet_is_refused`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "chains are read by the launch from an anchor (Task A5)"
    )
)]
fn check_name(name: &str) -> Result<(), String> {
    if name == "{launch}" || name == "{from}" {
        return Ok(());
    }
    if let Some((space, kept)) = RESERVED.iter().find(|(space, _)| name.starts_with(space)) {
        return Err(format!("\"{name}\": {space} names are kept for {kept}"));
    }
    match name
        .split('/')
        .find(|segment| !in_alphabet(&segment.replace("{id}", "0").replace("{app_id}", "a")))
    {
        Some(segment) => Err(format!(
            "\"{name}\": the segment \"{segment}\" is not letters, digits and . _ % ~ -"
        )),
        None => Ok(()),
    }
}

/// What a chain is resolved for: the flying window's script id and
/// application id, its launch's chain, what an unminimize was asked from,
/// and the monitor the request came from.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "chains are resolved for the launch from an anchor (Task A5)"
    )
)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Context<'a> {
    pub(crate) id: Option<u64>,
    pub(crate) app_id: Option<&'a str>,
    pub(crate) launch: Option<&'a Chain>,
    pub(crate) from: Option<&'a Chain>,
    pub(crate) output: Option<&'a str>,
}

/// Fill a chain's templates, once, when its flight starts: `{id}` and
/// `{app_id}` in every name, and a link that is exactly `{launch}` or
/// `{from}` replaced by that chain's links. A template with nothing to fill
/// it takes its link out.
/// `tests::templates_expand_from_the_window`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "chains are expanded for the launch from an anchor (Task A5)"
    )
)]
pub(crate) fn expand(chain: &Chain, ctx: &Context<'_>) -> Chain {
    let mut links = Vec::new();
    for link in &chain.0 {
        let Link::Name(name) = link else {
            links.push(link.clone());
            continue;
        };
        match name.as_str() {
            "{launch}" => links.extend(ctx.launch.iter().flat_map(|chain| chain.0.iter().cloned())),
            "{from}" => links.extend(ctx.from.iter().flat_map(|chain| chain.0.iter().cloned())),
            _ => {
                let mut filled = name.clone();
                if filled.contains("{id}") {
                    let Some(id) = ctx.id else { continue };
                    filled = filled.replace("{id}", &id.to_string());
                }
                if filled.contains("{app_id}") {
                    let Some(app_id) = ctx.app_id else { continue };
                    filled = filled.replace("{app_id}", app_id);
                }
                links.push(Link::Name(filled));
            }
        }
    }
    Chain(links)
}

/// What resolving needs of the desktop: where an owner's scene is, the
/// monitors, the panes and the surfaces, and the settings.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "resolved for the launch from an anchor (Task A5)")
)]
pub(crate) trait Places {
    /// Where an owner's scene is drawn, on the desktop.
    fn scene_origin(&self, owner: &Owner) -> Option<Point<f64, Logical>>;
    /// A monitor by connector name, `active` or `primary`, and its rectangle.
    fn monitor(&self, which: &str, ctx: &Context<'_>) -> Option<(String, Rect)>;
    /// One rule of `anchors.same`: the monitor it names for this flight.
    fn monitor_by(&self, how: Same, ctx: &Context<'_>) -> Option<String>;
    fn settings(&self) -> &Settings;
    /// A pane by its script id, where it is drawn.
    fn pane(&self, id: u64) -> Option<Rect>;
    /// A `sol.surface` by name, its instance for this flight.
    fn surface(&self, name: &str, ctx: &Context<'_>) -> Option<Rect>;
}

/// The built-in edge strip's shape, `anchors.edge`.
/// `tests::the_monitor_names_resolve_to_a_monitor_or_the_middle_third_of_an_edge`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Edge {
    /// Logical pixels, a whole number from 1 to 512.
    pub(crate) depth: f64,
    /// A share of the edge, above 0 and at most 1.
    pub(crate) length: f64,
}

impl Default for Edge {
    fn default() -> Self {
        Self {
            depth: 24.0,
            length: 1.0 / 3.0,
        }
    }
}

/// One rule of `anchors.same`: which monitor's copy of a name a flight
/// means. `tests::same_follows_anchors_same`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Same {
    /// The monitor under the flying window's centre.
    Pane,
    /// The monitor the request names: the sending scene's, for an action.
    Sender,
    /// The monitor under the pointer.
    Pointer,
    /// The active monitor.
    Active,
    /// The primary monitor.
    Primary,
}

impl Same {
    const WORDS: [(&'static str, Self); 5] = [
        ("pane", Self::Pane),
        ("sender", Self::Sender),
        ("pointer", Self::Pointer),
        ("active", Self::Active),
        ("primary", Self::Primary),
    ];
}

/// `config.anchors`: the edge strip, which monitor a name means, and the
/// opacity above which a region counts as shown.
/// `tests::anchors_settings_are_bounded`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) edge: Edge,
    pub(crate) same: Vec<Same>,
    pub(crate) shown_opacity: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            edge: Edge::default(),
            same: vec![Same::Pane, Same::Sender, Same::Active, Same::Primary],
            shown_opacity: 1.0 / 255.0,
        }
    }
}

impl Settings {
    /// Read the section, a key left out keeping its default. A value outside
    /// its bounds is refused naming the key and the bounds.
    /// `tests::anchors_settings_are_bounded`.
    pub(crate) fn read(
        depth: Option<f64>,
        length: Option<f64>,
        same: Option<&[String]>,
        shown_opacity: Option<f64>,
    ) -> Result<Self, String> {
        let mut settings = Self::default();
        if let Some(depth) = depth {
            if !(1.0..=512.0).contains(&depth) || depth.fract() != 0.0 {
                return Err(format!(
                    "anchors.edge.depth must be a whole number of logical pixels from 1 to 512, not {depth}"
                ));
            }
            settings.edge.depth = depth;
        }
        if let Some(length) = length {
            if !(length > 0.0 && length <= 1.0) {
                return Err(format!(
                    "anchors.edge.length must be a share of the edge above 0 and at most 1, not {length}"
                ));
            }
            settings.edge.length = length;
        }
        if let Some(words) = same {
            const BOUNDS: &str = "anchors.same must list one to five of \"pane\", \"sender\", \
                                  \"pointer\", \"active\" and \"primary\", none twice";
            if words.is_empty() || words.len() > Same::WORDS.len() {
                return Err(format!("{BOUNDS}; it lists {}", words.len()));
            }
            let mut rules = Vec::with_capacity(words.len());
            for word in words {
                let Some((_, rule)) = Same::WORDS.iter().find(|(name, _)| name == word) else {
                    return Err(format!("{BOUNDS}; \"{word}\" is none of them"));
                };
                if rules.contains(rule) {
                    return Err(format!("{BOUNDS}; \"{word}\" is there twice"));
                }
                rules.push(*rule);
            }
            settings.same = rules;
        }
        if let Some(opacity) = shown_opacity {
            if !(0.0..=1.0).contains(&opacity) {
                return Err(format!(
                    "anchors.shown_opacity must be from 0 to 1, not {opacity}"
                ));
            }
            settings.shown_opacity = opacity;
        }
        Ok(settings)
    }
}

/// Which monitor's copy of a name a flight means: the first entry of
/// `anchors.same` that answers, by default the pane's own by its centre,
/// then the sender's, the active and the primary (Ruling M3).
/// `tests::same_follows_anchors_same`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "resolved for the launch from an anchor (Task A5)")
)]
pub(crate) fn same_monitor(ctx: &Context<'_>, places: &impl Places) -> Option<String> {
    places
        .settings()
        .same
        .iter()
        .find_map(|how| places.monitor_by(*how, ctx))
}

/// What a chain resolved to this pass: the rectangle on the desktop, where
/// the region was in its scene's last render (a departure starts there), its
/// radii, which link it was, the region's key and generation (none for a
/// built-in name or today's aims), and whether its pixels could be taken.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "resolved for the launch from an anchor (Task A5)")
)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Resolved {
    pub(crate) rect: Rect,
    pub(crate) rendered: Rect,
    pub(crate) radius: [f32; 4],
    pub(crate) entry: u8,
    pub(crate) key: Option<Key>,
    pub(crate) generation: u64,
    pub(crate) shown: bool,
}

/// Resolve a chain this pass: the first link that names something, where it
/// is, its radius, and which link it was.
///
/// A region resolves when it is published on the flight's monitor, shown or
/// not (`tests::a_published_region_resolves_though_it_is_not_shown`): a
/// hidden dock's icon is still where a minimise goes. Its `rendered` place is
/// where a departure starts (`tests::a_departure_starts_where_the_region_was_last_drawn`).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "resolved for the launch from an anchor (Task A5)")
)]
pub(crate) fn resolve(
    chain: &Chain,
    ctx: &Context<'_>,
    registry: &Registry,
    names: &Names,
    places: &impl Places,
) -> Option<Resolved> {
    chain.0.iter().enumerate().find_map(|(entry, link)| {
        let entry = u8::try_from(entry).unwrap_or(u8::MAX);
        let flat = |rect: Rect| Resolved {
            rect,
            rendered: rect,
            radius: [0.0; 4],
            entry,
            key: None,
            generation: 0,
            shown: true,
        };
        match link {
            Link::Rect(rect) => Some(flat(*rect)),
            Link::Window(id) => places.pane(*id).map(flat),
            Link::Surface(name) => places.surface(name, ctx).map(flat),
            Link::Name(name) => {
                if let Some(rest) = name.strip_prefix("monitor/") {
                    return monitor_link(rest, ctx, places).map(flat);
                }
                if let Some(id) = name.strip_prefix("pane/") {
                    return id.parse().ok().and_then(|id| places.pane(id)).map(flat);
                }
                let key = names.find(name)?;
                // Which monitor's copy: `anchors.same`, walked in order.
                // `tests::same_follows_anchors_same`.
                let output = same_monitor(ctx, places)?;
                let (owner, published) = registry.on(key, &output)?;
                let origin = places.scene_origin(owner)?;
                let global = |local: Rect| {
                    Rect::new(
                        (origin.x + local.loc.x, origin.y + local.loc.y).into(),
                        local.size,
                    )
                };
                Some(Resolved {
                    rect: global(published.local),
                    rendered: global(published.rendered),
                    radius: published.radius,
                    entry,
                    key: Some(key),
                    generation: published.generation,
                    shown: published.shown,
                })
            }
        }
    })
}

/// `monitor/<which>[/<edge>]`: a monitor by connector, `same`, `active` or
/// `primary`, or a strip of one of its edges.
/// `tests::the_monitor_names_resolve_to_a_monitor_or_the_middle_third_of_an_edge`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "resolved for the launch from an anchor (Task A5)")
)]
fn monitor_link(rest: &str, ctx: &Context<'_>, places: &impl Places) -> Option<Rect> {
    let (which, edge) = match rest.split_once('/') {
        Some((which, edge)) => (which, Some(edge)),
        None => (rest, None),
    };
    let (_, monitor) = if which == "same" {
        places.monitor(&same_monitor(ctx, places)?, ctx)?
    } else {
        places.monitor(which, ctx)?
    };
    match edge {
        None => Some(monitor),
        Some(edge) => edge_strip(monitor, edge, places.settings().edge),
    }
}

/// A strip of a monitor's edge, centred along it and inside the monitor:
/// `anchors.edge`'s share of the edge's length, `depth` deep, by default
/// the middle third and 24 px (Ruling M2).
/// `tests::the_monitor_names_resolve_to_a_monitor_or_the_middle_third_of_an_edge`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "resolved for the launch from an anchor (Task A5)")
)]
fn edge_strip(monitor: Rect, edge: &str, shape: Edge) -> Option<Rect> {
    let (x, y, w, h) = (monitor.loc.x, monitor.loc.y, monitor.size.w, monitor.size.h);
    let (along_w, along_h) = (w * shape.length, h * shape.length);
    let (sx, sy, sw, sh) = match edge {
        "top" => (x + (w - along_w) / 2.0, y, along_w, shape.depth),
        "bottom" => (
            x + (w - along_w) / 2.0,
            y + h - shape.depth,
            along_w,
            shape.depth,
        ),
        "left" => (x, y + (h - along_h) / 2.0, shape.depth, along_h),
        "right" => (
            x + w - shape.depth,
            y + (h - along_h) / 2.0,
            shape.depth,
            along_h,
        ),
        _ => return None,
    };
    Some(Rect::new((sx, sy).into(), (sw, sh).into()))
}

#[cfg(test)]
mod tests {
    use super::{
        Chain, Context, Edge, Given, Link, Names, Owner, Places, Rect, RegionChange, Registry,
        Same, Settings, expand, parse_chain, resolve,
    };
    use crate::scripted::SurfaceId;
    use smithay::utils::{Logical, Point};

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new((x, y).into(), (w, h).into())
    }

    fn text(name: &str) -> Given {
        Given::Text(name.to_owned())
    }

    /// A change of a shown region, drawn where it is.
    fn change(path: &str, local: Rect) -> RegionChange {
        RegionChange {
            path: path.to_owned(),
            gone: false,
            local,
            rendered: local,
            radius: [0.0; 4],
            shown: true,
            rendered_pass: 1,
        }
    }

    fn published(
        registry: &mut Registry,
        names: &mut Names,
        owner: Owner,
        path: &str,
        local: Rect,
    ) {
        registry.apply(names, &owner, "shell", &[change(path, local)]);
    }

    /// Two monitors, `DP-1` at 0,0 2560×1440 and `DP-2` at 2560,0
    /// 1920×1080, the shell's instance on each at its monitor's origin,
    /// window 7 on DP-1 and window 9 on DP-2, DP-1 active and primary, no
    /// pointer.
    struct Desk {
        monitors: Vec<(&'static str, Rect)>,
        windows: Vec<(u64, Rect)>,
        settings: Settings,
    }

    impl Desk {
        fn new() -> Self {
            Self {
                monitors: vec![
                    ("DP-1", rect(0.0, 0.0, 2560.0, 1440.0)),
                    ("DP-2", rect(2560.0, 0.0, 1920.0, 1080.0)),
                ],
                windows: vec![
                    (7, rect(100.0, 100.0, 800.0, 600.0)),
                    (9, rect(2700.0, 100.0, 800.0, 600.0)),
                ],
                settings: Settings::default(),
            }
        }

        fn shell(&self, output: &str) -> Owner {
            Owner {
                surface: SurfaceId::from_raw(0),
                output: output.to_owned(),
            }
        }

        fn on<'a>(&self, output: &'a str) -> Context<'a> {
            Context {
                id: None,
                app_id: None,
                launch: None,
                from: None,
                output: Some(output),
            }
        }

        fn rect_of(&self, name: &str) -> Option<Rect> {
            self.monitors
                .iter()
                .find(|(each, _)| *each == name)
                .map(|(_, rect)| *rect)
        }
    }

    impl Places for Desk {
        fn scene_origin(&self, owner: &Owner) -> Option<Point<f64, Logical>> {
            self.rect_of(&owner.output).map(|rect| rect.loc)
        }

        fn monitor(&self, which: &str, _ctx: &Context<'_>) -> Option<(String, Rect)> {
            let name = match which {
                "active" | "primary" => "DP-1",
                other => other,
            };
            self.rect_of(name).map(|rect| (name.to_owned(), rect))
        }

        fn monitor_by(&self, how: Same, ctx: &Context<'_>) -> Option<String> {
            match how {
                Same::Pane => {
                    let (_, window) = self.windows.iter().find(|(id, _)| Some(*id) == ctx.id)?;
                    let centre = (
                        window.loc.x + window.size.w / 2.0,
                        window.loc.y + window.size.h / 2.0,
                    );
                    self.monitors
                        .iter()
                        .find(|(_, rect)| rect.contains(centre))
                        .map(|(name, _)| (*name).to_owned())
                }
                Same::Sender => ctx.output.map(str::to_owned),
                Same::Pointer => None,
                Same::Active | Same::Primary => Some("DP-1".to_owned()),
            }
        }

        fn settings(&self) -> &Settings {
            &self.settings
        }

        fn pane(&self, id: u64) -> Option<Rect> {
            self.windows
                .iter()
                .find(|(each, _)| *each == id)
                .map(|(_, rect)| *rect)
        }

        fn surface(&self, name: &str, ctx: &Context<'_>) -> Option<Rect> {
            (name == "shell")
                .then(|| super::same_monitor(ctx, self))
                .flatten()
                .and_then(|output| self.rect_of(&output))
        }
    }

    /// **A name resolves on the flight's own monitor first**: the shell is
    /// one instance per monitor, so `shell/dock/icons/foot` is two regions,
    /// and a window on DP-2 flies to DP-2's (#143's rule, for names).
    #[test]
    fn a_name_resolves_on_the_flights_own_monitor_first() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-1"),
            "dock/icons/foot",
            rect(100.0, 6.0, 40.0, 40.0),
        );
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-2"),
            "dock/icons/foot",
            rect(80.0, 6.0, 40.0, 40.0),
        );
        let chain = parse_chain(&text("shell/dock/icons/foot")).expect("a name");
        let resolved =
            resolve(&chain, &desk.on("DP-2"), &registry, &names, &desk).expect("resolves");
        assert_eq!(resolved.rect, rect(2560.0 + 80.0, 6.0, 40.0, 40.0));
        assert_eq!(resolved.entry, 0);
    }

    /// **A chain takes its first link that resolves**, and says which.
    #[test]
    fn a_chain_takes_its_first_link_that_resolves() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-1"),
            "dock/icons/foot",
            rect(100.0, 6.0, 40.0, 40.0),
        );
        let chain = parse_chain(&Given::Otherwise(
            Box::new(text("shell/bar/windows/7")),
            Box::new(text("shell/dock/icons/foot")),
        ))
        .expect("a chain");
        let resolved =
            resolve(&chain, &desk.on("DP-1"), &registry, &names, &desk).expect("the second link");
        assert_eq!((resolved.entry, resolved.rect.loc.x), (1, 100.0));
        let list = parse_chain(&Given::List(vec![
            text("shell/bar/windows/7"),
            text("shell/dock/icons/foot"),
            text("monitor/same/bottom"),
        ]))
        .expect("a list");
        assert_eq!(
            resolve(&list, &desk.on("DP-1"), &registry, &names, &desk).map(|r| r.entry),
            Some(1)
        );
    }

    /// **A published region resolves though it is not shown** (Ruling M3):
    /// a hidden dock's icon is still where a flight goes, and the flight
    /// follows it as the dock reveals.
    #[test]
    fn a_published_region_resolves_though_it_is_not_shown() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        let mut hidden = change("dock/icons/foot", rect(100.0, -60.0, 40.0, 40.0));
        hidden.shown = false;
        registry.apply(&mut names, &desk.shell("DP-1"), "shell", &[hidden]);
        let chain = parse_chain(&text("shell/dock/icons/foot")).expect("a name");
        let resolved =
            resolve(&chain, &desk.on("DP-1"), &registry, &names, &desk).expect("resolves");
        assert!(!resolved.shown && resolved.rect.loc.y < 0.0);
    }

    /// **A departure starts where the region was last drawn**: `rendered`,
    /// not where a later polish has already moved it.
    #[test]
    fn a_departure_starts_where_the_region_was_last_drawn() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        let mut moved = change("search/rows/foot", rect(400.0, 900.0, 600.0, 40.0));
        moved.rendered = rect(400.0, 300.0, 600.0, 40.0);
        registry.apply(&mut names, &desk.shell("DP-1"), "shell", &[moved]);
        let chain = parse_chain(&text("shell/search/rows/foot")).expect("a name");
        let resolved =
            resolve(&chain, &desk.on("DP-1"), &registry, &names, &desk).expect("resolves");
        assert_eq!(resolved.rendered.loc.y, 300.0);
        assert_eq!(resolved.rect.loc.y, 900.0);
    }

    /// **Templates expand from the window**: `{launch}` splices in the
    /// launch's own chain and `{from}` what an unminimize was asked from, each
    /// or nothing when there was none.
    #[test]
    fn templates_expand_from_the_window() {
        let launch = parse_chain(&text("shell/dock/icons/foot")).expect("a name");
        let clicked = parse_chain(&text("shell/dock/icons/kitty")).expect("a name");
        let chain = parse_chain(&Given::List(vec![
            text("{from}"),
            text("{launch}"),
            text("shell/bar/windows/{id}"),
            text("shell/dock/icons/{app_id}"),
        ]))
        .expect("a list");
        let ctx = Context {
            id: Some(7),
            app_id: Some("foot"),
            launch: Some(&launch),
            from: Some(&clicked),
            output: None,
        };
        assert_eq!(
            expand(&chain, &ctx),
            Chain(vec![
                Link::Name("shell/dock/icons/kitty".into()),
                Link::Name("shell/dock/icons/foot".into()),
                Link::Name("shell/bar/windows/7".into()),
                Link::Name("shell/dock/icons/foot".into()),
            ])
        );
        let neither = Context {
            launch: None,
            from: None,
            ..ctx
        };
        assert_eq!(expand(&chain, &neither).0.len(), 2);
        let no_window = Context {
            id: None,
            app_id: None,
            ..ctx
        };
        assert_eq!(
            expand(&chain, &no_window).0.len(),
            2,
            "a template with nothing to fill it takes its link out"
        );
    }

    /// **`anchors.same` says which monitor's copy of a name a flight means**:
    /// window 9 lives on DP-2, so by default it flies to DP-2's icon; with
    /// `{ "active" }` and DP-1 active it flies to DP-1's.
    #[test]
    fn same_follows_anchors_same() {
        let (mut registry, mut names, mut desk) =
            (Registry::default(), Names::default(), Desk::new());
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-1"),
            "dock/icons/foot",
            rect(100.0, 6.0, 40.0, 40.0),
        );
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-2"),
            "dock/icons/foot",
            rect(80.0, 6.0, 40.0, 40.0),
        );
        let chain = parse_chain(&text("shell/dock/icons/foot")).expect("a name");
        let nine = Context {
            id: Some(9),
            app_id: None,
            launch: None,
            from: None,
            output: None,
        };
        let x = |desk: &Desk| {
            resolve(&chain, &nine, &registry, &names, desk)
                .expect("resolves")
                .rect
                .loc
                .x
        };
        assert_eq!(
            x(&desk),
            2560.0 + 80.0,
            "the default is the pane's own monitor"
        );
        desk.settings.same = vec![Same::Active];
        assert_eq!(x(&desk), 100.0, "anchors.same was not followed");
    }

    /// **The monitor names resolve to a monitor, or a strip of its edge**:
    /// `monitor/same` is the flying window's own monitor before the sender's,
    /// an edge is by default its middle third 24 px deep, and `anchors.edge`
    /// changes the strip.
    #[test]
    fn the_monitor_names_resolve_to_a_monitor_or_the_middle_third_of_an_edge() {
        let (registry, names, mut desk) = (Registry::default(), Names::default(), Desk::new());
        let at = |desk: &Desk, name: &str, ctx: &Context<'_>| {
            resolve(
                &parse_chain(&text(name)).expect("a name"),
                ctx,
                &registry,
                &names,
                desk,
            )
            .map(|resolved| resolved.rect)
        };
        let nine_sent_from_one = Context {
            id: Some(9),
            ..desk.on("DP-1")
        };
        assert_eq!(
            at(&desk, "monitor/same", &nine_sent_from_one),
            Some(rect(2560.0, 0.0, 1920.0, 1080.0)),
            "monitor/same is the window's own monitor, before the sender's"
        );
        assert_eq!(
            at(&desk, "monitor/same", &desk.on("DP-2")),
            Some(rect(2560.0, 0.0, 1920.0, 1080.0))
        );
        let bottom = at(&desk, "monitor/DP-1/bottom", &desk.on("DP-1")).expect("an edge");
        let third = 2560.0 / 3.0;
        assert!((bottom.loc.x - third).abs() < 1e-9 && (bottom.size.w - third).abs() < 1e-9);
        assert_eq!((bottom.loc.y, bottom.size.h), (1416.0, 24.0));
        assert_eq!(
            at(&desk, "monitor/primary/left", &desk.on("DP-2")),
            Some(rect(0.0, 480.0, 24.0, 480.0))
        );
        assert_eq!(at(&desk, "monitor/DP-1/middle", &desk.on("DP-1")), None);
        desk.settings.edge = Edge {
            depth: 40.0,
            length: 0.5,
        };
        assert_eq!(
            at(&desk, "monitor/DP-1/bottom", &desk.on("DP-1")),
            Some(rect(640.0, 1400.0, 1280.0, 40.0)),
            "anchors.edge was not followed"
        );
    }

    /// **`config.anchors` is bounded**: each value outside its bounds is
    /// refused naming the key and the bounds, and nothing given is the
    /// defaults.
    #[test]
    fn anchors_settings_are_bounded() {
        let words = |list: &[&str]| {
            list.iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        let refused = |result: Result<Settings, String>| result.expect_err("refused");
        for depth in [0.0, 600.0, 2.5] {
            let why = refused(Settings::read(Some(depth), None, None, None));
            assert!(
                why.contains("anchors.edge.depth") && why.contains("1 to 512"),
                "{why}"
            );
        }
        for length in [0.0, 1.5] {
            let why = refused(Settings::read(None, Some(length), None, None));
            assert!(
                why.contains("anchors.edge.length") && why.contains("at most 1"),
                "{why}"
            );
        }
        for same in [words(&[]), words(&["pane", "pane"]), words(&["left"])] {
            let why = refused(Settings::read(None, None, Some(&same), None));
            assert!(
                why.contains("anchors.same") && why.contains("\"primary\""),
                "{why}"
            );
        }
        let why = refused(Settings::read(None, None, None, Some(1.5)));
        assert!(
            why.contains("anchors.shown_opacity") && why.contains("0 to 1"),
            "{why}"
        );
        assert_eq!(
            Settings::read(None, None, None, None),
            Ok(Settings::default())
        );
        let read = Settings::read(
            Some(40.0),
            Some(0.5),
            Some(&words(&["pointer", "primary"])),
            Some(0.5),
        )
        .expect("in bounds");
        assert_eq!(
            read,
            Settings {
                edge: Edge {
                    depth: 40.0,
                    length: 0.5
                },
                same: vec![Same::Pointer, Same::Primary],
                shown_opacity: 0.5,
            }
        );
    }

    /// **A scene torn down takes its regions with it**, and only its own.
    #[test]
    fn a_scene_torn_down_takes_its_regions_with_it() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-1"),
            "dock/icons/foot",
            rect(100.0, 6.0, 40.0, 40.0),
        );
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-2"),
            "dock/icons/foot",
            rect(80.0, 6.0, 40.0, 40.0),
        );
        registry.drop_owner(&desk.shell("DP-1"));
        let key = names.find("shell/dock/icons/foot").expect("named");
        assert!(registry.on(key, "DP-1").is_none());
        assert!(registry.on(key, "DP-2").is_some());
        registry.retain_owners(|owner| owner.output != "DP-2");
        assert!(registry.is_empty());
    }

    /// **A gone change removes one region**, and leaves the rest.
    #[test]
    fn a_gone_change_removes_one_region() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        let owner = desk.shell("DP-1");
        registry.apply(
            &mut names,
            &owner,
            "shell",
            &[
                change("a", rect(0.0, 0.0, 1.0, 1.0)),
                change("b", rect(0.0, 0.0, 1.0, 1.0)),
            ],
        );
        let mut gone = change("a", rect(0.0, 0.0, 0.0, 0.0));
        gone.gone = true;
        registry.apply(&mut names, &owner, "shell", &[gone]);
        let left: Vec<&str> = registry
            .under(&names, "")
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(left, ["shell/b"]);
    }

    /// **Generations rise on every change**, and a change that changes
    /// nothing keeps its region's.
    #[test]
    fn generations_rise_on_every_change() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        let owner = desk.shell("DP-1");
        let generation = |registry: &Registry, names: &Names| {
            registry
                .on(names.find("shell/a").expect("named"), "DP-1")
                .map(|(_, published)| published.generation)
        };
        registry.apply(
            &mut names,
            &owner,
            "shell",
            &[change("a", rect(0.0, 0.0, 10.0, 10.0))],
        );
        let first = generation(&registry, &names);
        registry.apply(
            &mut names,
            &owner,
            "shell",
            &[change("a", rect(0.0, 0.0, 10.0, 10.0))],
        );
        assert_eq!(
            generation(&registry, &names),
            first,
            "the same values raised it"
        );
        registry.apply(
            &mut names,
            &owner,
            "shell",
            &[change("a", rect(5.0, 0.0, 10.0, 10.0))],
        );
        assert!(
            generation(&registry, &names) > first,
            "a move did not raise it"
        );
    }

    /// **Today's aims are links**: a window, a surface and a rectangle parse
    /// and resolve as `Solium::anchor_rect` resolves them, with no radius.
    #[test]
    fn today_s_aims_are_links() {
        let (registry, names, desk) = (Registry::default(), Names::default(), Desk::new());
        let ctx = desk.on("DP-2");
        let at = |given: Given| {
            resolve(
                &parse_chain(&given).expect("a link"),
                &ctx,
                &registry,
                &names,
                &desk,
            )
            .map(|resolved| (resolved.rect, resolved.radius, resolved.key))
        };
        assert_eq!(
            at(Given::Window(7)),
            Some((rect(100.0, 100.0, 800.0, 600.0), [0.0; 4], None))
        );
        assert_eq!(
            at(text("pane/9")),
            Some((rect(2700.0, 100.0, 800.0, 600.0), [0.0; 4], None))
        );
        assert_eq!(
            at(Given::Surface("shell".into())),
            Some((rect(2560.0, 0.0, 1920.0, 1080.0), [0.0; 4], None))
        );
        assert_eq!(
            at(Given::Rect(rect(1.0, 2.0, 3.0, 4.0))),
            Some((rect(1.0, 2.0, 3.0, 4.0), [0.0; 4], None))
        );
        assert_eq!(at(Given::Window(8)), None);
    }

    /// **A reserved namespace is refused by name**, each saying what it is
    /// kept for.
    #[test]
    fn a_reserved_namespace_is_refused_by_name() {
        for (name, item) in [
            ("lock/clock", "lock screen"),
            ("layer/waybar", "#50"),
            ("toplevel/3", "#50"),
            ("lua/x", "sol.anchor"),
        ] {
            let why = parse_chain(&text(name)).expect_err("reserved");
            assert!(why.contains(name) && why.contains(item), "{why}");
        }
    }

    /// **A path out of its alphabet is refused**, quoting the segment.
    #[test]
    fn a_path_out_of_its_alphabet_is_refused() {
        let why = parse_chain(&text("shell/dock/icons/a b")).expect_err("a space");
        assert!(why.contains("\"a b\""), "{why}");
        assert!(parse_chain(&text("shell//x")).is_err(), "an empty segment");
        assert!(parse_chain(&text("shell/desktop/notes%20%281%29.txt")).is_ok());
    }

    /// **A region whose path is out of its alphabet is skipped by the
    /// registry** (Ruling M2), and the scene's other regions are kept.
    #[test]
    fn a_path_out_of_its_alphabet_is_skipped_by_the_registry() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        registry.apply(
            &mut names,
            &desk.shell("DP-1"),
            "shell",
            &[
                change("dock/a b", rect(0.0, 0.0, 1.0, 1.0)),
                change("dock/b", rect(0.0, 0.0, 1.0, 1.0)),
            ],
        );
        let kept: Vec<&str> = registry
            .under(&names, "")
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(kept, ["shell/dock/b"]);
    }

    /// **A nested `otherwise` reads as one list**, in order.
    #[test]
    fn nested_otherwise_reads_as_one_list() {
        let chain = parse_chain(&Given::Otherwise(
            Box::new(Given::List(vec![
                text("shell/a"),
                Given::Otherwise(Box::new(text("shell/b")), Box::new(text("shell/c"))),
            ])),
            Box::new(Given::Window(4)),
        ))
        .expect("a chain");
        assert_eq!(
            chain,
            Chain(vec![
                Link::Name("shell/a".into()),
                Link::Name("shell/b".into()),
                Link::Name("shell/c".into()),
                Link::Window(4),
            ])
        );
    }

    /// **`under` lists a prefix by name and monitor**, for `sol.anchors`.
    #[test]
    fn under_lists_a_prefix_by_name_and_monitor() {
        let (mut registry, mut names, desk) = (Registry::default(), Names::default(), Desk::new());
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-2"),
            "dock/b",
            rect(0.0, 0.0, 1.0, 1.0),
        );
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-1"),
            "dock/b",
            rect(0.0, 0.0, 1.0, 1.0),
        );
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-1"),
            "dock/a",
            rect(0.0, 0.0, 1.0, 1.0),
        );
        published(
            &mut registry,
            &mut names,
            desk.shell("DP-1"),
            "bar/x",
            rect(0.0, 0.0, 1.0, 1.0),
        );
        let listed: Vec<(&str, &str)> = registry
            .under(&names, "shell/dock/")
            .map(|(name, owner, _)| (name, owner.output.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("shell/dock/a", "DP-1"),
                ("shell/dock/b", "DP-1"),
                ("shell/dock/b", "DP-2")
            ]
        );
    }
}
