//! Surfaces a script declares, drawn by the compositor in QML.
//!
//! One primitive for everything the compositor draws that is not a window and
//! not a client: a wallpaper, a bar, a dock, a heads-up display, a debug
//! overlay. A script names a QML file, says which layer and which monitors,
//! and hands over some properties; the compositor rasterises it and puts it in
//! the frame.
//!
//! ## Why this exists rather than a wallpaper
//!
//! The wallpaper was written first, the narrow way: a `Command::Wallpaper`, an
//! accessor on `Solium`, a special case in `render.rs`, a hundred lines of
//! Rust for a picture behind the windows. It worked, and it was the wrong
//! shape by this project's own test — `docs/modes.md` says that if a new thing
//! needs new Rust, the layer underneath is missing something.
//!
//! It also demonstrated the wrong claim. A built-in wallpaper says the
//! compositor has a wallpaper. This says anything QML can draw can be part of
//! the desktop without touching Rust, which is the claim the project actually
//! makes, and the wallpaper is now its first and smallest proof: `lua/wallpaper.lua`
//! is nine lines and there is no wallpaper code in the compositor at all.
//!
//! ## What is deliberately not here
//!
//! Most input. A surface is drawn and not clicked unless it is declared
//! `interactive`, and then it gets plain pointer motion and presses, which is
//! enough for the tweaks panel's buttons. Keyboard focus, grabs and everything
//! else a real client gets are a bigger question than this — they need the
//! scoped grab in #85. A hosted shell is one of these surfaces and has the
//! same limits (`docs/shell-boundary.md`, "What it is not given"); until #85,
//! a surface that needs a keyboard has to be a layer-shell client.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

/// A surface's identity, as everything that is not a script holds it.
///
/// **A number rather than the name the script wrote**, and the reason is one
/// type away: `present::Frame` is `Copy`, and so is everything reachable from
/// it — the anchor a deformation aims at lives inside one, a `Frame` is copied
/// out of a `RefCell` and blended for every animating node on every frame, and
/// the slot it lives in is `RefCell<Option<Transform<Frame>>>`. A
/// `Box<str>` in there is a heap allocation in the value the render loop copies
/// per node per frame, paid by every window so that one of them can name a
/// dock. Four bytes is not.
///
/// The other way out was to make `Frame` clone-not-copy, which is the same cost
/// arrived at by a longer route, through `present.rs`, `render.rs`, `state.rs`
/// and `script.rs`.
///
/// Assigned by [`Surfaces`] and **stable across a redeclaration**: running the
/// configuration again replaces a surface with an equal one, and a genie aimed
/// at the dock should not stop being aimed at it because `super+shift+r` was
/// pressed. Never reused, for the reason `PaneId` is never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SurfaceId(u32);

impl SurfaceId {
    /// For tests in other modules, which need an id without a live surface
    /// behind it. Nothing outside a test should be inventing one of these:
    /// [`Surfaces::id_of`] is where they come from.
    #[cfg(test)]
    pub(crate) const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

use smithay::{
    output::Output,
    utils::{Logical, Rectangle},
};

use crate::{json::Json, surface::ShellSurface};

/// Where a surface sits in the frame.
///
/// Named after the wlr-layer-shell layers on purpose, and each one sits
/// *under* the matching layer of real client surfaces — so a `swaybg` on the
/// background layer covers a scripted background, and a real bar covers a
/// scripted one. A compositor's own furniture yielding to a client's is the
/// right way round: the client was installed on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Layer {
    /// Under everything, including client background surfaces.
    #[default]
    Background,
    /// Above the background, below windows.
    Bottom,
    /// Above windows, below client top and overlay surfaces -- and below a
    /// fullscreen window, which is lifted over the bars (`crate::stack`).
    Top,
    /// Above client top surfaces, below client overlay ones. Below the
    /// pointer, which is above everything.
    Overlay,
}

impl Layer {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "background" => Some(Self::Background),
            "bottom" => Some(Self::Bottom),
            "top" => Some(Self::Top),
            "overlay" => Some(Self::Overlay),
            _ => None,
        }
    }
}

/// Which monitors a surface is drawn on, and how big it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum On {
    /// One instance per monitor, filling it.
    EveryMonitor,
    /// One instance, filling the primary monitor.
    Primary,
    /// One instance, filling the monitor with this connector name.
    Monitor(String),
    /// Exactly this rectangle in the global space, on whichever monitors it
    /// overlaps.
    Rect(Rectangle<i32, Logical>),
}

/// What a script asked for. Carried from Lua to the compositor, so it holds no
/// rasterisations and can be cloned like every other command.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Declaration {
    pub(crate) name: String,
    pub(crate) scene: PathBuf,
    pub(crate) layer: Layer,
    pub(crate) on: On,
    /// The property bag.
    pub(crate) properties: Properties,
    /// Whether the pointer reaches it.
    ///
    /// Off by default, and that is the safe default rather than the tidy one:
    /// a full-screen background surface that took the pointer would swallow
    /// every click on the desktop, and the symptom would be "windows stopped
    /// responding" rather than anything mentioning wallpapers.
    pub(crate) interactive: bool,
}

/// A surface's property bag, as values.
///
/// Sorted by key, so the same table declared twice is the same bag whatever
/// order Lua walked it in: `tests::properties_render_in_key_order`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Properties(BTreeMap<String, Json>);

impl Properties {
    pub(crate) fn new(values: BTreeMap<String, Json>) -> Self {
        Self(values)
    }

    /// The whole bag as one JSON object: what a scene is built with.
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        crate::json::write_object(&self.0, &mut out);
        out
    }

    #[cfg(test)]
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        self.0.get(key)
    }

    /// The keys whose value is new or different, with their values. A key
    /// `old` has and this does not is not a change: the scene keeps it
    /// (Ruling 3). `tests::only_changed_and_added_keys_are_changes`,
    /// `tests::a_table_a_list_and_a_dotted_key_reach_the_live_scene`.
    pub(crate) fn changes_from(&self, old: &Self) -> Vec<(String, Json)> {
        self.0
            .iter()
            .filter(|(key, value)| old.0.get(*key) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

/// What a declaration did. `tests::a_redeclared_property_is_written_into_the_live_scene`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Declared {
    /// Equal to what was declared: nothing to do.
    Same,
    /// The same scene: changed properties were written into the live scenes.
    InPlace,
    /// A different scene file: the old scenes are gone.
    Rebuilt,
    /// A name never declared before, or declared and removed.
    Added,
}

impl Declaration {
    /// A declaration with every option at its default, for tests.
    #[cfg(test)]
    pub(crate) fn for_test(name: &str, scene: PathBuf, layer: Layer, on: On) -> Self {
        Self {
            name: name.to_owned(),
            scene,
            layer,
            on,
            properties: Properties::default(),
            interactive: true,
        }
    }
}

/// A declaration, plus what has been rasterised for it.
#[derive(Debug)]
pub(crate) struct Surface {
    pub(crate) declared: Declaration,
    /// What names this to anything that cannot hold a `String`. See
    /// [`SurfaceId`].
    id: SurfaceId,
    /// One rasterisation per monitor it is drawn on, keyed by connector name.
    ///
    /// Per monitor because a `ShellSurface` caches one rasterisation at one
    /// size: two screens of different sizes sharing one would re-rasterise a
    /// full-screen scene twice a frame, for ever.
    instances: HashMap<String, ShellSurface>,
}

impl Surface {
    pub(crate) fn new(declared: Declaration, id: SurfaceId) -> Self {
        Self {
            declared,
            id,
            instances: HashMap::new(),
        }
    }

    pub(crate) const fn id(&self) -> SurfaceId {
        self.id
    }

    pub(crate) fn name(&self) -> &str {
        &self.declared.name
    }

    pub(crate) fn layer(&self) -> Layer {
        self.declared.layer
    }

    pub(crate) fn interactive(&self) -> bool {
        self.declared.interactive
    }

    /// Offer the pointer to this surface's instance on one monitor.
    ///
    /// Returns whether it was inside. A surface that takes the pointer stops
    /// it reaching anything underneath, which is what makes a button a button.
    pub(crate) fn pointer(
        &mut self,
        output: &Output,
        area: Rectangle<i32, Logical>,
        x: f64,
        y: f64,
        pressed: Option<bool>,
    ) -> bool {
        self.instance(output)
            .is_some_and(|instance| instance.pointer(area, x, y, pressed))
    }

    /// Whatever the scene asked for since it was last looked at.
    ///
    /// The same one-way channel the window frames and the tweaks panel use:
    /// QML sets `action`, the compositor takes it and clears it, so a press is
    /// acted on once. Asked of every monitor's instance because the press
    /// landed on exactly one of them and this does not know which.
    pub(crate) fn taken_action(&mut self) -> Option<String> {
        self.instances
            .values_mut()
            .find_map(crate::surface::ShellSurface::taken_action)
    }

    /// Where this surface goes on one monitor, if it goes there at all.
    pub(crate) fn area_on(
        &self,
        output: &Output,
        geometry: Rectangle<i32, Logical>,
        primary: Option<&Output>,
    ) -> Option<Rectangle<i32, Logical>> {
        match &self.declared.on {
            On::EveryMonitor => Some(geometry),
            On::Primary => (Some(output) == primary).then_some(geometry),
            On::Monitor(name) => (&output.name() == name).then_some(geometry),
            // Clipped to the monitor, so a rect spanning two screens is drawn
            // on both and each gets its own share rather than the whole thing
            // twice.
            On::Rect(rect) => geometry.intersection(*rect),
        }
    }

    /// The rasterisation for one monitor, built on first use and hosted on
    /// that monitor. `tests::a_surface_instance_is_hosted_on_its_monitor`.
    pub(crate) fn instance(&mut self, output: &Output) -> Option<&mut ShellSurface> {
        let name = output.name();
        if !self.instances.contains_key(&name) {
            match ShellSurface::hosted(
                self.declared.scene.clone(),
                &self.declared.properties.render(),
                &name,
            ) {
                Ok(surface) => {
                    self.instances.insert(name.clone(), surface);
                }
                Err(err) => {
                    // Once per monitor, not once per frame: a scene that will
                    // not load is a line in the log and a gap in the picture,
                    // not a session that stops.
                    tracing::error!(
                        ?err,
                        surface = self.declared.name,
                        scene = %self.declared.scene.display(),
                        "that surface would not load"
                    );
                    return None;
                }
            }
        }
        self.instances.get_mut(&name)
    }

    /// Forget every instance on a monitor this surface is no longer on: one
    /// that has gone, one its placement has moved off, and the old primary
    /// for a surface on the primary monitor. Each is a whole scene, kept
    /// undrawn otherwise until that monitor is unplugged.
    /// `tests::a_surface_moved_to_another_monitor_drops_the_scene_it_left`,
    /// `tests::a_surface_on_the_primary_monitor_drops_its_scene_when_the_primary_moves`.
    pub(crate) fn keep_placed(
        &mut self,
        outputs: &[(Output, Rectangle<i32, Logical>)],
        primary: Option<&Output>,
    ) {
        let placed: Vec<String> = outputs
            .iter()
            .filter(|(output, geometry)| self.area_on(output, *geometry, primary).is_some())
            .map(|(output, _)| output.name())
            .collect();
        self.instances
            .retain(|monitor, _| placed.iter().any(|name| name == monitor));
    }

    #[cfg(test)]
    pub(crate) fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Take a declaration with the same scene: write its changed properties
    /// into every live instance, and keep everything else of the instances.
    /// Each instance's rebuild bag becomes the new one even when nothing was
    /// written, as when a key was only dropped. Instances on a monitor the
    /// new placement leaves are the caller's to drop ([`Self::keep_placed`]).
    /// `tests::a_redeclared_property_is_written_into_the_live_scene`,
    /// `tests::a_table_a_list_and_a_dotted_key_reach_the_live_scene`,
    /// `tests::a_changed_placement_keeps_the_live_scene`,
    /// `tests::a_redeclaration_that_only_drops_keys_still_updates_the_rebuild_bag`.
    fn update(&mut self, declared: Declaration) {
        if declared.properties != self.declared.properties {
            let changed = declared.properties.changes_from(&self.declared.properties);
            let bag = declared.properties.render();
            for instance in self.instances.values_mut() {
                instance.set_properties(&bag, &changed);
            }
        }
        self.declared = declared;
    }
}

/// Every surface a script has declared, and the names they answer to.
///
/// A wrapper around what used to be a plain `Vec<Surface>`, and it earns the
/// wrapping by holding the one thing a `Vec` cannot: **the name table**. A
/// script names a surface with a string; a `present::Anchor` and a
/// `group::Member` have to name one with something `Copy`. Keeping the
/// assignment beside the surfaces is what makes an id stable across a surface
/// being replaced, which a monotonic counter inside `Surface::new` would not be.
///
/// The table only grows, and what it grows by is names a *configuration* wrote
/// down: one entry per distinct surface name the session has ever seen, a few
/// bytes each. It is not fed by anything a client or a running program can
/// drive.
#[derive(Debug, Default)]
pub(crate) struct Surfaces {
    live: Vec<Surface>,
    /// Every name ever declared, in id order. The index *is* the id.
    names: Vec<Box<str>>,
}

impl Surfaces {
    /// The id for a name, assigning one if this is the first time it has been
    /// seen. Only the declaration path should intern; everything else asks
    /// [`Self::named`] and accepts that a name nobody declared names nothing.
    fn intern(&mut self, name: &str) -> SurfaceId {
        if let Some(id) = self.named(name) {
            return id;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "one entry per distinct surface name a configuration has written; \
                      four billion of them is not a case"
        )]
        let id = SurfaceId(self.names.len() as u32);
        self.names.push(name.into());
        id
    }

    /// What a name means, or nothing if no surface has ever had it.
    pub(crate) fn named(&self, name: &str) -> Option<SurfaceId> {
        self.names
            .iter()
            .position(|each| &**each == name)
            .and_then(|index| u32::try_from(index).ok())
            .map(SurfaceId)
    }

    /// Declare a surface, or change the one with that name.
    ///
    /// Only a different scene file replaces a surface; anything else changes
    /// the live scenes in place, so an open popup, a running animation and a
    /// half-typed query survive a reload (primitive 1).
    /// `tests::a_new_scene_path_rebuilds_the_surface`.
    pub(crate) fn declare(&mut self, declared: Declaration) -> Declared {
        let id = self.intern(&declared.name);
        match self.live.iter_mut().find(|each| each.id == id) {
            Some(existing) if existing.declared == declared => Declared::Same,
            Some(existing) if existing.declared.scene != declared.scene => {
                *existing = Surface::new(declared, id);
                Declared::Rebuilt
            }
            Some(existing) => {
                existing.update(declared);
                Declared::InPlace
            }
            None => {
                self.live.push(Surface::new(declared, id));
                Declared::Added
            }
        }
    }

    /// Take one away, by name. Returns whether there was one.
    pub(crate) fn remove(&mut self, name: &str) -> bool {
        let before = self.live.len();
        self.live.retain(|surface| surface.name() != name);
        before != self.live.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Surface> {
        self.live.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Surface> {
        self.live.iter_mut()
    }

    pub(crate) fn get_mut(&mut self, id: SurfaceId) -> Option<&mut Surface> {
        self.live.iter_mut().find(|surface| surface.id == id)
    }

    pub(crate) fn get(&self, id: SurfaceId) -> Option<&Surface> {
        self.live.iter().find(|surface| surface.id == id)
    }
}

/// Find a scene on the QML search path.
///
/// A bare name is looked up the way an `import` would be -- the user's
/// directory first, then the one that ships -- so dropping
/// `~/.config/solium/qml/wallpaper.qml` in place replaces the shipped scene
/// without copying anything else, which is the same rule decorations follow.
/// A path, absolute or `~`-prefixed, is taken as given.
pub(crate) fn find_scene(name: &str) -> Option<PathBuf> {
    if let Some(rest) = name.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return Some(PathBuf::from(home).join(rest));
    }
    if name.starts_with('/') {
        return Some(PathBuf::from(name));
    }
    if let Some(user) = crate::qml::user_qml_dir() {
        let path = user.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    let own = crate::assets::qml().join(name);
    own.is_file().then_some(own)
}

/// One JSON string, quoted and escaped.
///
/// The property bag is JSON text and the values in it come from a script,
/// which means from a user. An unescaped quote in a file name ends the string
/// early and takes the whole scene with it -- which presents as the surface
/// silently not existing.
pub(crate) fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
    use smithay::utils::{Logical, Rectangle};

    use super::{Declaration, Declared, Layer, On, Properties, Surfaces};
    use crate::json::Json;
    use crate::qml::qt_test::on_the_qt_thread;

    fn properties(pairs: &[(&str, Json)]) -> Properties {
        Properties::new(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    /// A 1920x1080 monitor at 1x, with no display behind it.
    pub(super) fn output(name: &str) -> Output {
        let output = Output::new(
            name.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_owned(),
                model: name.to_owned(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        output
    }

    fn object(pairs: &[(&str, Json)]) -> Json {
        Json::Object(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    /// Every monitor and its rectangle, as `Surface::keep_placed` takes them.
    type Monitors = Vec<(Output, Rectangle<i32, Logical>)>;

    /// Two monitors side by side, and the placement `Surface::keep_placed`
    /// is given for them.
    fn side_by_side(left: &str, right: &str) -> (Output, Output, Monitors) {
        let (left, right) = (output(left), output(right));
        let outputs = vec![
            (
                left.clone(),
                Rectangle::new((0, 0).into(), (1920, 1080).into()),
            ),
            (
                right.clone(),
                Rectangle::new((1920, 0).into(), (1920, 1080).into()),
            ),
        ];
        (left, right, outputs)
    }

    /// `qml` written to `<temp>/<name>/<file>`, and its path.
    pub(super) fn scene_file(name: &str, file: &str, qml: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(name);
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join(file);
        std::fs::write(&path, qml).expect("writing the scene");
        path
    }

    const KEPT: &str = r#"
        import QtQuick
        Item {
            property string label: ""
            property int kept: 0
            readonly property int labelIsTwo: label === "two" ? 1 : 0
        }
    "#;

    /// A scene with one property of every kind a declaration writes.
    const VALUES: &str = r#"
        import QtQuick
        Item {
            property var info: ({})
            property QtObject panel: QtObject {
                property bool open: false
                property int size: 0
            }
            property var list: []
            property int count: 0
            property var nothing: 1
            readonly property int nothingIsNull: nothing === null ? 1 : 0
            readonly property int infoWidth: info.width !== undefined ? info.width : -1
            readonly property int infoHeight: info.height !== undefined ? info.height : -1
            readonly property int infoDepth: info.inner !== undefined ? info.inner.depth : -1
            readonly property int panelOpen: panel.open ? 1 : 0
            readonly property int listLength: list.length
            readonly property int listLast: list.length > 0 ? list[list.length - 1] : -1
        }
    "#;

    #[test]
    fn properties_render_in_key_order() {
        let bag = properties(&[
            ("width", Json::Number(48.0)),
            ("label", Json::Text("a".to_owned())),
        ]);
        assert_eq!(bag.render(), r#"{"label":"a","width":48}"#);
    }

    #[test]
    fn only_changed_and_added_keys_are_changes() {
        let old = properties(&[("a", Json::Number(1.0)), ("b", Json::Number(2.0))]);
        let new = properties(&[
            ("a", Json::Number(1.0)),
            ("b", Json::Number(3.0)),
            ("c", Json::Bool(true)),
        ]);
        assert_eq!(
            new.changes_from(&old),
            vec![
                ("b".to_owned(), Json::Number(3.0)),
                ("c".to_owned(), Json::Bool(true))
            ]
        );
        assert!(new.changes_from(&new).is_empty());
    }

    /// **A redeclared property reaches the live scene, and the scene is the
    /// same scene**: what it holds survives. Primitive 1.
    #[test]
    fn a_redeclared_property_is_written_into_the_live_scene() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file("solium-scripted-in-place", "Scene.qml", KEPT);
            let screen = output("in-place-1");
            let mut surfaces = Surfaces::default();
            let mut declared = Declaration::for_test("bar", path, Layer::Top, On::EveryMonitor);
            declared.properties = properties(&[("label", Json::Text("one".to_owned()))]);
            assert_eq!(surfaces.declare(declared.clone()), Declared::Added);
            let id = surfaces.named("bar").expect("declared");
            surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("the scene builds")
                .scene_for_test()
                .set_int("kept", 7);

            declared.properties = properties(&[("label", Json::Text("two".to_owned()))]);
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let scene = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("still there")
                .scene_for_test();
            assert_eq!(
                scene.get_int("labelIsTwo"),
                1,
                "the new label did not reach the scene"
            );
            assert_eq!(
                scene.get_int("kept"),
                7,
                "the scene was rebuilt: what it held is gone"
            );
        });
    }

    /// **A changed placement keeps the live scene**: `on`, `layer` and
    /// `interactive` are the compositor's, and a scene does not hear of them.
    #[test]
    fn a_changed_placement_keeps_the_live_scene() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file("solium-scripted-placement", "Scene.qml", KEPT);
            let screen = output("placement-1");
            let mut surfaces = Surfaces::default();
            let mut declared = Declaration::for_test("bar", path, Layer::Top, On::EveryMonitor);
            surfaces.declare(declared.clone());
            let id = surfaces.named("bar").expect("declared");
            surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("the scene builds")
                .scene_for_test()
                .set_int("kept", 7);

            declared.on = On::Monitor("placement-1".to_owned());
            declared.layer = Layer::Overlay;
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let kept = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("still there")
                .scene_for_test()
                .get_int("kept");
            assert_eq!(kept, 7, "a new placement rebuilt the scene");
        });
    }

    /// **A new scene file rebuilds**, which is the one change that must.
    #[test]
    fn a_new_scene_path_rebuilds_the_surface() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let first = scene_file("solium-scripted-rebuild", "First.qml", KEPT);
            let second = scene_file("solium-scripted-rebuild", "Second.qml", KEPT);
            let screen = output("rebuild-1");
            let mut surfaces = Surfaces::default();
            let mut declared = Declaration::for_test("bar", first, Layer::Top, On::EveryMonitor);
            surfaces.declare(declared.clone());
            let id = surfaces.named("bar").expect("declared");
            surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("the scene builds")
                .scene_for_test()
                .set_int("kept", 7);

            declared.scene = second;
            assert_eq!(surfaces.declare(declared), Declared::Rebuilt);
            let kept = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("rebuilt")
                .scene_for_test()
                .get_int("kept");
            assert_eq!(kept, 0, "the old scene is still there");
        });
    }

    /// **Every kind of value reaches the live scene** (Ruling 3): a number
    /// into an `int`, a table written whole as a JavaScript object and never
    /// merged into the old one, a list, `null`, and a dotted key into a
    /// grouped property. A key the declaration drops keeps what the scene last had.
    #[test]
    fn a_table_a_list_and_a_dotted_key_reach_the_live_scene() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file("solium-scripted-values", "Scene.qml", VALUES);
            let screen = output("values-1");
            let mut surfaces = Surfaces::default();
            let mut declared = Declaration::for_test("bar", path, Layer::Top, On::EveryMonitor);
            declared.properties = properties(&[(
                "info",
                object(&[
                    ("height", Json::Number(200.0)),
                    ("width", Json::Number(320.0)),
                ]),
            )]);
            surfaces.declare(declared.clone());
            let id = surfaces.named("bar").expect("declared");
            assert!(
                surfaces
                    .get_mut(id)
                    .and_then(|surface| surface.instance(&screen))
                    .is_some(),
                "the scene builds"
            );

            declared.properties = properties(&[
                ("count", Json::Number(5.0)),
                (
                    "info",
                    object(&[
                        ("inner", object(&[("depth", Json::Number(3.0))])),
                        ("width", Json::Number(640.0)),
                    ]),
                ),
                (
                    "list",
                    Json::List(vec![
                        Json::Number(1.0),
                        Json::Number(2.0),
                        Json::Number(3.0),
                    ]),
                ),
                ("nothing", Json::Null),
                ("panel.open", Json::Bool(true)),
                ("panel.size", Json::Number(9.0)),
            ]);
            assert_eq!(surfaces.declare(declared.clone()), Declared::InPlace);
            let scene = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("still there")
                .scene_for_test();
            assert_eq!(scene.get_int("count"), 5, "a number did not reach an int");
            assert_eq!(scene.get_int("infoWidth"), 640, "the table did not arrive");
            assert_eq!(
                scene.get_int("infoHeight"),
                -1,
                "the table was merged into the old one rather than written whole"
            );
            assert_eq!(
                scene.get_int("infoDepth"),
                3,
                "a nested table did not arrive as an object"
            );
            assert_eq!(
                (scene.get_int("listLength"), scene.get_int("listLast")),
                (3, 3),
                "the list did not arrive"
            );
            assert_eq!(
                (scene.get_int("panelOpen"), scene.get_int("panel.size")),
                (1, 9),
                "a dotted key did not reach the grouped property"
            );
            assert_eq!(scene.get_int("nothingIsNull"), 1, "null did not arrive");

            declared.properties = properties(&[("count", Json::Number(6.0))]);
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let scene = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("still there")
                .scene_for_test();
            assert_eq!(scene.get_int("count"), 6);
            assert_eq!(
                (scene.get_int("infoWidth"), scene.get_int("panelOpen")),
                (640, 1),
                "a key the declaration dropped lost the value the scene had"
            );
        });
    }

    /// **A dotted key reaches a scene as it is built**, not only a live one:
    /// the first instance, and one built later on another monitor, after a
    /// redeclaration that left the dotted key as it was.
    #[test]
    fn a_dotted_key_reaches_a_freshly_built_scene() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file("solium-scripted-dotted", "Scene.qml", VALUES);
            let (left, right, _) = side_by_side("dotted-left", "dotted-right");
            let mut surfaces = Surfaces::default();
            let mut declared = Declaration::for_test("bar", path, Layer::Top, On::EveryMonitor);
            declared.properties = properties(&[
                ("count", Json::Number(4.0)),
                ("panel.open", Json::Bool(true)),
            ]);
            surfaces.declare(declared.clone());
            let id = surfaces.named("bar").expect("declared");
            let scene = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&left))
                .expect("the scene builds")
                .scene_for_test();
            assert_eq!(
                (scene.get_int("count"), scene.get_int("panelOpen")),
                (4, 1),
                "the first instance was built without its dotted key"
            );

            declared.properties = properties(&[
                ("count", Json::Number(5.0)),
                ("panel.open", Json::Bool(true)),
            ]);
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let scene = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&right))
                .expect("the scene builds on the second monitor")
                .scene_for_test();
            assert_eq!(
                (scene.get_int("count"), scene.get_int("panelOpen")),
                (5, 1),
                "the second monitor's instance was built without its dotted key"
            );
        });
    }

    /// **A surface moved to another monitor leaves nothing on the one it
    /// left**, where its whole scene would otherwise sit undrawn until that
    /// monitor is unplugged; the monitor it moved to builds a scene from the
    /// current properties.
    #[test]
    fn a_surface_moved_to_another_monitor_drops_the_scene_it_left() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file("solium-scripted-moved", "Scene.qml", KEPT);
            let (left, right, outputs) = side_by_side("moved-left", "moved-right");
            let mut surfaces = Surfaces::default();
            let mut declared = Declaration::for_test(
                "bar",
                path,
                Layer::Top,
                On::Monitor("moved-left".to_owned()),
            );
            declared.properties = properties(&[("label", Json::Text("one".to_owned()))]);
            surfaces.declare(declared.clone());
            let id = surfaces.named("bar").expect("declared");
            surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&left))
                .expect("the scene builds")
                .scene_for_test()
                .set_int("kept", 7);

            declared.on = On::Monitor("moved-right".to_owned());
            declared.properties = properties(&[("label", Json::Text("two".to_owned()))]);
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let surface = surfaces.get_mut(id).expect("live");
            surface.keep_placed(&outputs, Some(&left));
            assert_eq!(
                surface.instance_count(),
                0,
                "the monitor it left kept its scene"
            );
            let scene = surface
                .instance(&right)
                .expect("the scene builds where it moved to")
                .scene_for_test();
            assert_eq!(
                (scene.get_int("labelIsTwo"), scene.get_int("kept")),
                (1, 0),
                "the new monitor's scene is built from the current properties"
            );
        });
    }

    /// **A surface on the primary monitor leaves nothing on the old one when
    /// the primary moves**: the same leak, reached by a monitor change rather
    /// than a declaration.
    #[test]
    fn a_surface_on_the_primary_monitor_drops_its_scene_when_the_primary_moves() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file("solium-scripted-primary-moved", "Scene.qml", KEPT);
            let (left, right, outputs) = side_by_side("primary-left", "primary-right");
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test("bar", path, Layer::Top, On::Primary));
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            assert!(surface.instance(&left).is_some(), "the scene builds");
            surface.keep_placed(&outputs, Some(&left));
            assert_eq!(
                surface.instance_count(),
                1,
                "the primary monitor lost its scene"
            );
            surface.keep_placed(&outputs, Some(&right));
            assert_eq!(
                surface.instance_count(),
                0,
                "the monitor that stopped being primary kept its scene"
            );
        });
    }

    /// **The bag a scene is rebuilt with is always the declaration's**, even
    /// when a redeclaration only drops keys and writes nothing into the
    /// scene: an edit to the QML builds from what is declared now.
    #[test]
    fn a_redeclaration_that_only_drops_keys_still_updates_the_rebuild_bag() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file("solium-scripted-dropped", "Scene.qml", KEPT);
            let screen = output("dropped-1");
            let mut surfaces = Surfaces::default();
            let mut declared = Declaration::for_test("bar", path, Layer::Top, On::EveryMonitor);
            declared.properties = properties(&[
                ("kept", Json::Number(3.0)),
                ("label", Json::Text("one".to_owned())),
            ]);
            surfaces.declare(declared.clone());
            let id = surfaces.named("bar").expect("declared");
            assert!(
                surfaces
                    .get_mut(id)
                    .and_then(|surface| surface.instance(&screen))
                    .is_some(),
                "the scene builds"
            );

            declared.properties = properties(&[("label", Json::Text("one".to_owned()))]);
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let bag = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("still there")
                .properties_for_test()
                .to_owned();
            assert_eq!(bag, r#"{"label":"one"}"#);
        });
    }

    /// **A surface's instance is hosted on its monitor**: `Solium.monitor`
    /// inside its scene names the monitor the instance was built for.
    /// Primitive 2.
    #[test]
    fn a_surface_instance_is_hosted_on_its_monitor() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file(
                "solium-scripted-hosted",
                "Scene.qml",
                r#"
                import QtQuick
                import Solium
                Item {
                    readonly property int named: Solium.monitor.name === "hosted-1" ? 1 : 0
                }
                "#,
            );
            let screen = output("hosted-1");
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test(
                "bar",
                path,
                Layer::Top,
                On::EveryMonitor,
            ));
            let id = surfaces.named("bar").expect("declared");
            let named = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance(&screen))
                .expect("the scene builds")
                .scene_for_test()
                .get_int("named");
            assert_eq!(
                named, 1,
                "the instance's scene does not know the monitor it is on"
            );
        });
    }

    #[test]
    fn an_identical_declaration_is_the_same() {
        let mut surfaces = Surfaces::default();
        let declared = Declaration::for_test(
            "bar",
            Path::new("/nonexistent/bar.qml").to_owned(),
            Layer::Top,
            On::EveryMonitor,
        );
        assert_eq!(surfaces.declare(declared.clone()), Declared::Added);
        assert_eq!(surfaces.declare(declared), Declared::Same);
    }
}
