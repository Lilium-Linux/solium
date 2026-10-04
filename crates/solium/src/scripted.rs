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
//! `interactive`, and then it gets pointer motion, every button, the wheel and
//! the modifiers held
//! (`state::tests::real_client::reflow_on_close::hosted::a_right_press_on_a_scene_reaches_it_as_the_right_button_with_shift_held`,
//! `state::tests::real_client::reflow_on_close::hosted::the_wheel_over_a_scene_reaches_it`),
//! where its scene's items take input, and the rest goes to what is under it
//! (`state::tests::real_client::reflow_on_close::hosted::a_press_where_the_shell_draws_nothing_reaches_the_window_under_it`).
//! A `Grab` in its scene holds the pointer for it, and a press outside the
//! grab's target dismisses it and is swallowed or passed on as the surface's
//! `outside_click` says
//! (`state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`).
//! An item of its scene that asks for the keyboard holds it, and the keys
//! reach it as its surface's `keyboard.bindings` says
//! (`input::tests::a_claimed_key_reaches_the_scene_and_not_its_binding`,
//! `state::tests::real_client::reflow_on_close::hosted::the_window_gets_the_keyboard_back_when_the_shell_lets_go`).
//! A hosted shell is one of these surfaces and has the same limits
//! (`docs/shell-boundary.md`, "What it is not given").

use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    utils::{Logical, Point, Rectangle},
};

use crate::{
    json::Json,
    qml::hosted::{GrabReport, Hit, KeyboardReport, SceneKey, ScenePointer},
    surface::ShellSurface,
};

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
    /// What it takes out of the work area of every monitor it is on, per
    /// edge, whatever its size or placement (#162, Ruling 10).
    /// `state::tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`.
    pub(crate) reserve: Edges,
    /// What a press outside an open grab of its scene does once it has
    /// dismissed it (Ruling 13).
    /// `state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`.
    pub(crate) outside_click: OutsideClick,
    /// Which compositor bindings still work while its scene holds the
    /// keyboard (Q3, Ruling 14).
    /// `input::tests::a_claimed_key_reaches_the_scene_and_not_its_binding`.
    pub(crate) keyboard: KeyPolicy,
}

/// Which compositor bindings still work while a scene holds the keyboard:
/// every one but the keys the holding item claims (the default), every one,
/// or none, so the scene has every key but the escape hatches (Q3).
/// `input::tests::a_claimed_key_reaches_the_scene_and_not_its_binding`,
/// `input::tests::with_bindings_all_a_claimed_binding_wins`,
/// `input::tests::with_bindings_none_even_super_bindings_reach_the_scene`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum KeyPolicy {
    #[default]
    ExceptClaimed,
    All,
    NoBindings,
}

/// What a press outside a grab does after dismissing it: swallowed, as macOS
/// and iOS do, or passed on to what is under it (Q2, Ruling 13).
/// `state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`,
/// `state::tests::real_client::reflow_on_close::hosted::with_outside_click_pass_the_dismissing_press_reaches_the_window_under_it`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Outside {
    #[default]
    Swallow,
    Pass,
}

/// A surface's outside-press policy: one for every grab, and one per grab
/// name that has its own.
/// `script::tests::sol_surface_reads_outside_click_as_a_word_or_a_table`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OutsideClick {
    pub(crate) default: Outside,
    pub(crate) named: BTreeMap<String, Outside>,
}

impl OutsideClick {
    /// What a press outside the grab named `name` does: its own entry, else
    /// the default.
    /// `script::tests::sol_surface_reads_outside_click_as_a_word_or_a_table`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_policy_named_for_the_grab_beats_the_default`.
    pub(crate) fn for_grab(&self, name: &str) -> Outside {
        self.named.get(name).copied().unwrap_or(self.default)
    }
}

/// Logical pixels on each edge of a monitor.
/// `state::monitors::tests::a_reserve_adds_to_the_layer_zone_on_its_edge`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Edges {
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
    pub(crate) left: i32,
}

impl Edges {
    /// Both, edge by edge: two bars on one edge reserve the two together.
    /// `state::tests::real_client::reflow_on_close::hosted::two_surfaces_reserving_one_edge_take_both`.
    /// Saturating: `state::monitors::tests::a_reserve_too_large_for_any_monitor_leaves_one_pixel`.
    pub(crate) fn add(self, other: Self) -> Self {
        Self {
            top: self.top.saturating_add(other.top),
            right: self.right.saturating_add(other.right),
            bottom: self.bottom.saturating_add(other.bottom),
            left: self.left.saturating_add(other.left),
        }
    }
}

/// What a scene says it reserves, per edge: `None` for an edge it has not
/// set, which the declaration keeps (Ruling 10).
/// `qml::hosted::tests::a_scene_reserve_is_reported_once_per_change`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SceneReserve {
    pub(crate) top: Option<i32>,
    pub(crate) right: Option<i32>,
    pub(crate) bottom: Option<i32>,
    pub(crate) left: Option<i32>,
}

impl SceneReserve {
    /// Per edge, the scene's value once it has set one, else the declared
    /// one (Ruling 10).
    /// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`.
    pub(crate) fn over(self, declared: Edges) -> Edges {
        let edge = |scene: Option<i32>, declared: i32| scene.map_or(declared, |value| value.max(0));
        Edges {
            top: edge(self.top, declared.top),
            right: edge(self.right, declared.right),
            bottom: edge(self.bottom, declared.bottom),
            left: edge(self.left, declared.left),
        }
    }
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

    /// Every value, for `scenario`.
    #[cfg(test)]
    pub(crate) const fn fields(&self) -> &BTreeMap<String, Json> {
        &self.0
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
            reserve: Edges::default(),
            outside_click: OutsideClick::default(),
            keyboard: KeyPolicy::default(),
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
    /// One instance per monitor it is on, keyed by connector name, each
    /// hosted on its monitor
    /// (`tests::a_surface_on_every_monitor_has_one_live_scene_per_monitor`).
    ///
    /// Per monitor because a `ShellSurface` caches one rasterisation at one
    /// size: two screens of different sizes sharing one would re-rasterise a
    /// full-screen scene twice a frame, for ever.
    instances: HashMap<String, ShellSurface>,
    /// The monitors its scene would not load on, not tried again until the
    /// surface names another scene file
    /// (`tests::a_scene_that_will_not_load_waits_for_another_scene_file`) or
    /// the configuration is reloaded ([`Self::forget_failures`]).
    failed: HashSet<String>,
    /// Whether the scene file not being there has been reported.
    missing_logged: bool,
    /// What answers for its scene in a state test, which cannot build one
    /// beside a real Wayland client: see [`Stand`].
    #[cfg(test)]
    stand: Option<Stand>,
    /// The reserve its stand-in last reported.
    #[cfg(test)]
    stood_reserve: SceneReserve,
}

impl Surface {
    pub(crate) fn new(declared: Declaration, id: SurfaceId) -> Self {
        Self {
            declared,
            id,
            instances: HashMap::new(),
            failed: HashSet::new(),
            missing_logged: false,
            #[cfg(test)]
            stand: None,
            #[cfg(test)]
            stood_reserve: SceneReserve::default(),
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

    /// Deliver one pointer event to this surface's instance on one monitor.
    /// True when there was one to take it.
    /// `tests::a_delivered_press_reaches_the_instance_in_its_own_coordinates`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_right_press_on_a_scene_reaches_it_as_the_right_button_with_shift_held`.
    pub(crate) fn deliver(
        &mut self,
        output: &Output,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
        event: ScenePointer,
    ) -> bool {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_mut() {
            stand.seen.push((location - area.loc.to_f64(), event));
            return true;
        }
        self.instance_mut(output).is_some_and(|instance| {
            instance.pointer(area, location, &event);
            true
        })
    }

    /// What this surface's scene on one monitor claims at a point, nothing
    /// outside its area.
    /// `state::tests::real_client::reflow_on_close::hosted::a_press_where_the_shell_draws_nothing_reaches_the_window_under_it`.
    pub(crate) fn hit(
        &self,
        output: &Output,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
    ) -> Hit {
        if !area.to_f64().contains(location) {
            return Hit::Nothing;
        }
        #[cfg(test)]
        if let Some(stand) = self.stand.as_ref() {
            return (stand.hit)(location);
        }
        self.instances
            .get(&output.name())
            .map_or(Hit::Nothing, |instance| instance.hit(area, location))
    }

    /// Tell this surface's scene on one monitor that the pointer left it.
    /// `state::tests::real_client::reflow_on_close::hosted::the_scene_hears_the_pointer_leave_when_it_moves_off_its_items`.
    pub(crate) fn leave(&mut self, output: &Output) {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_mut() {
            stand.left += 1;
            return;
        }
        if let Some(instance) = self.instance_mut(output) {
            instance.leave();
        }
    }

    /// Stand `stand` in for this surface's scene on every monitor.
    #[cfg(test)]
    pub(crate) fn stand_in(&mut self, stand: Stand) {
        self.stand = Some(stand);
    }

    /// What stands in for its scene, and what reached it.
    #[cfg(test)]
    pub(crate) fn stand(&self) -> Option<&Stand> {
        self.stand.as_ref()
    }

    /// The same, to change what it reports.
    #[cfg(test)]
    pub(crate) fn stand_mut(&mut self) -> Option<&mut Stand> {
        self.stand.as_mut()
    }

    /// Read what every instance's scene reserves now.
    /// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`.
    pub(crate) fn take_reserves(&mut self) {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_mut() {
            if let Some(reserve) = stand.reserve.take() {
                self.stood_reserve = reserve;
            }
            return;
        }
        for instance in self.instances.values_mut() {
            instance.take_reserve();
        }
    }

    /// What this surface reserves on one monitor: the declaration, under what
    /// the instance there has set. The declaration counts whether or not the
    /// scene loaded (Ruling 10).
    /// `state::tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`.
    pub(crate) fn reserve_on(&self, output: &Output) -> Edges {
        #[cfg(test)]
        if self.stand.is_some() {
            return self.stood_reserve.over(self.declared.reserve);
        }
        self.instances
            .get(&output.name())
            .map_or(self.declared.reserve, |instance| {
                instance.scene_reserve().over(self.declared.reserve)
            })
    }

    /// What each instance's scene says of its grabs since it was last asked,
    /// by the name of its monitor, leaving out those with nothing new. `on`
    /// is every monitor the surface is on, which a stand-in reports for the
    /// first of.
    /// `state::tests::real_client::reflow_on_close::hosted::a_grab_another_scene_takes_dismisses_the_one_held`.
    pub(crate) fn take_grabs(&mut self, on: &[String]) -> Vec<(String, GrabReport)> {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_mut() {
            return match (stand.grab.take(), on.first()) {
                (Some(report), Some(monitor)) => vec![(monitor.clone(), report)],
                _ => Vec::new(),
            };
        }
        #[cfg(not(test))]
        let _ = on;
        self.instances
            .iter_mut()
            .map(|(monitor, instance)| (monitor.clone(), instance.take_grab()))
            .filter(|(_, report)| *report != GrabReport::Unchanged)
            .collect()
    }

    /// What each instance's scene says of its keyboard wants since it was
    /// last asked, by the name of its monitor, leaving out those with
    /// nothing new. `on` is every monitor the surface is on, which a
    /// stand-in reports for the first of.
    /// `state::tests::real_client::reflow_on_close::hosted::the_window_gets_the_keyboard_back_when_the_shell_lets_go`.
    pub(crate) fn take_keyboards(&mut self, on: &[String]) -> Vec<(String, KeyboardReport)> {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_mut() {
            return match (stand.keyboard.take(), on.first()) {
                (Some(report), Some(monitor)) => vec![(monitor.clone(), report)],
                _ => Vec::new(),
            };
        }
        #[cfg(not(test))]
        let _ = on;
        self.instances
            .iter_mut()
            .map(|(monitor, instance)| (monitor.clone(), instance.take_keyboard()))
            .filter(|(_, report)| *report != KeyboardReport::Unchanged)
            .collect()
    }

    /// Tell this surface's scene on one monitor one key.
    /// `input::tests::russian_typed_through_the_compositor_reaches_a_hosted_text_field`.
    pub(crate) fn key(&mut self, output: &Output, key: &SceneKey) {
        #[cfg(test)]
        if self.stand.is_some() {
            return;
        }
        if let Some(instance) = self.instance_mut(output) {
            instance.key(key);
        }
    }

    /// Tell this surface's scene on one monitor that the compositor took the
    /// keyboard back.
    /// `state::tests::real_client::reflow_on_close::hosted::clicking_a_window_ends_the_shells_hold`.
    pub(crate) fn let_go_keyboard(&mut self, output: &Output) {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_mut() {
            stand.let_go += 1;
            return;
        }
        if let Some(instance) = self.instance_mut(output) {
            instance.let_go_keyboard();
        }
    }

    /// Whether a point in compositor coordinates is inside an active grab's
    /// target of this surface's scene on one monitor, drawn across `area`.
    /// `state::tests::real_client::reflow_on_close::hosted::a_press_inside_the_grab_target_reaches_the_scene`.
    pub(crate) fn grab_contains(
        &self,
        output: &Output,
        area: Rectangle<i32, Logical>,
        location: Point<f64, Logical>,
    ) -> bool {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_ref() {
            return (stand.inside)(location);
        }
        self.instances
            .get(&output.name())
            .is_some_and(|instance| instance.grab_contains(area, location))
    }

    /// Dismiss the grabs of this surface's scene on one monitor.
    /// `state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`.
    pub(crate) fn dismiss(&mut self, output: &Output) {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_mut() {
            stand.dismissed += 1;
            return;
        }
        if let Some(instance) = self.instance_mut(output) {
            instance.dismiss();
        }
    }

    /// Whether it has a scene on one monitor to hold a grab with.
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_taken_away_lets_go_of_its_grab`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_scene_that_goes_from_a_monitor_lets_go_of_its_grab`.
    pub(crate) fn hosts_on(&self, output: &Output) -> bool {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_ref() {
            return stand.hosts;
        }
        self.instances.contains_key(&output.name())
    }

    /// Every action its scenes asked for since they were last looked at,
    /// each instance's in order, the instances by their monitors' names
    /// (Ruling 15).
    /// `state::tests::real_client::reflow_on_close::hosted::two_actions_from_one_frame_both_reach_lua_in_order`,
    /// `tests::every_instances_actions_are_taken_by_monitor_name`.
    pub(crate) fn take_actions(&mut self) -> Vec<(String, Json)> {
        #[cfg(test)]
        if let Some(stand) = self.stand.as_mut() {
            return std::mem::take(&mut stand.actions);
        }
        let mut instances: Vec<(&String, &mut crate::surface::ShellSurface)> =
            self.instances.iter_mut().collect();
        instances.sort_by_key(|(monitor, _)| *monitor);
        instances
            .into_iter()
            .flat_map(|(_, instance)| instance.take_actions())
            .collect()
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

    /// Build an instance for every monitor this surface is on, and drop the
    /// rest. Called when the monitors are settled and when the surface is
    /// declared, rather than at the first frame that draws it. A monitor that
    /// has gone takes its instance with it, so a laptop docked and undocked
    /// all day does not keep a full-screen scene per screen it ever saw, and
    /// one the placement has moved off does too (Ruling 5).
    /// `tests::an_instance_goes_with_its_monitor_and_comes_with_a_new_one`,
    /// `tests::a_surface_moved_to_another_monitor_drops_the_scene_it_left`,
    /// `tests::a_surface_on_the_primary_monitor_drops_its_scene_when_the_primary_moves`.
    pub(crate) fn sync(
        &mut self,
        outputs: &[(Output, Rectangle<i32, Logical>)],
        primary: Option<&Output>,
    ) {
        let wanted: Vec<String> = outputs
            .iter()
            .filter(|(output, geometry)| self.area_on(output, *geometry, primary).is_some())
            .map(|(output, _)| output.name())
            .collect();
        self.instances.retain(|monitor, _| wanted.contains(monitor));
        self.failed.retain(|monitor| wanted.contains(monitor));
        // Asked before Qt is: a file that is not there is not a scene that
        // would not load, and the first sync after it appears builds it.
        // `tests::a_missing_scene_file_builds_nothing_until_it_is_there`.
        if !self.declared.scene.is_file() {
            if !self.missing_logged {
                self.missing_logged = true;
                tracing::error!(
                    surface = self.declared.name,
                    scene = %self.declared.scene.display(),
                    "no such QML scene"
                );
            }
            return;
        }
        if !builds_here() {
            return;
        }
        for monitor in wanted {
            if self.instances.contains_key(&monitor) || self.failed.contains(&monitor) {
                continue;
            }
            match ShellSurface::hosted(
                self.declared.scene.clone(),
                &self.declared.properties.render(),
                &monitor,
            ) {
                Ok(instance) => {
                    self.instances.insert(monitor, instance);
                }
                Err(err) => {
                    // Once per monitor, not once per sync.
                    // `tests::a_scene_that_will_not_load_waits_for_another_scene_file`.
                    tracing::error!(
                        ?err,
                        surface = self.declared.name,
                        scene = %self.declared.scene.display(),
                        monitor,
                        "that surface would not load"
                    );
                    self.failed.insert(monitor);
                }
            }
        }
    }

    /// Try its scene again, at the next sync, on every monitor it would not
    /// load on, and say again that a scene file is not there. A reload does
    /// (`state::tests::real_client::a_reload_tries_again_a_scene_that_would_not_load`).
    pub(crate) fn forget_failures(&mut self) {
        self.failed.clear();
        self.missing_logged = false;
    }

    /// The instance on one monitor, if it was built there.
    pub(crate) fn instance_mut(&mut self, output: &Output) -> Option<&mut ShellSurface> {
        self.instances.get_mut(&output.name())
    }

    #[cfg(test)]
    pub(crate) fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Take a declaration with the same scene: write its changed properties
    /// into every live instance, and keep everything else of the instances.
    /// Each instance's rebuild bag becomes the new one even when nothing was
    /// written, as when a key was only dropped. Instances on a monitor the
    /// new placement leaves are the caller's to drop, and on one it newly
    /// covers to build, from the current properties ([`Self::sync`]).
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

/// What stands in for a surface's scene in a state test, which cannot build a
/// Qt scene beside a real Wayland client (the #99 test): what it claims, and
/// what reached it. One stand answers for every instance of its surface.
/// `state::tests::real_client::reflow_on_close::hosted::a_right_press_on_a_scene_reaches_it_as_the_right_button_with_shift_held`.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Stand {
    /// What the scene claims at a point, in compositor coordinates.
    pub(crate) hit: fn(Point<f64, Logical>) -> Hit,
    /// Every pointer event delivered, in the surface's own coordinates.
    pub(crate) seen: Vec<(Point<f64, Logical>, ScenePointer)>,
    /// How many times the pointer was told it left.
    pub(crate) left: u32,
    /// The reserve the scene reports at the next settle, taken once.
    pub(crate) reserve: Option<SceneReserve>,
    /// The grab the scene reports at the next settle, taken once.
    pub(crate) grab: Option<GrabReport>,
    /// Whether a point, in compositor coordinates, is inside an active
    /// grab's target.
    pub(crate) inside: fn(Point<f64, Logical>) -> bool,
    /// How many times its grabs were dismissed.
    pub(crate) dismissed: u32,
    /// Whether it has a scene on the monitors it is on.
    pub(crate) hosts: bool,
    /// The keyboard wants the scene reports at the next settle, taken once.
    pub(crate) keyboard: Option<KeyboardReport>,
    /// How many times the compositor took the keyboard back from it.
    pub(crate) let_go: u32,
    /// Actions the scene queued, taken at the next settle.
    pub(crate) actions: Vec<(String, Json)>,
}

#[cfg(test)]
impl Stand {
    /// A scene that takes a press everywhere in its area, as every surface
    /// did before #173.
    pub(crate) fn solid() -> Self {
        Self {
            hit: |_| Hit::Press,
            seen: Vec::new(),
            left: 0,
            reserve: None,
            grab: None,
            inside: |_| false,
            dismissed: 0,
            hosts: true,
            keyboard: None,
            let_go: 0,
            actions: Vec::new(),
        }
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

/// Whether a scene may be built on this thread. In a test binary, only on the
/// Qt thread: `tests::a_surface_synced_off_the_qt_thread_builds_nothing`.
#[cfg(not(test))]
const fn builds_here() -> bool {
    true
}

#[cfg(test)]
fn builds_here() -> bool {
    crate::qml::qt_test::is_the_qt_thread()
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
    use crate::qml::hosted::{Hit, PointerKind, ScenePointer};
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

    /// Every monitor and its rectangle, as `Surface::sync` takes them.
    type Monitors = Vec<(Output, Rectangle<i32, Logical>)>;

    /// One monitor alone, as `Surface::sync` takes it.
    fn alone(screen: &Output) -> Monitors {
        vec![(
            screen.clone(),
            Rectangle::new((0, 0).into(), (1920, 1080).into()),
        )]
    }

    /// Two monitors side by side, and the placement `Surface::sync` is given
    /// for them.
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
                .expect("live")
                .sync(&alone(&screen), Some(&screen));
            surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&screen))
                .expect("the scene builds")
                .scene_for_test()
                .set_int("kept", 7);

            declared.properties = properties(&[("label", Json::Text("two".to_owned()))]);
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let scene = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&screen))
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
                .expect("live")
                .sync(&alone(&screen), Some(&screen));
            surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&screen))
                .expect("the scene builds")
                .scene_for_test()
                .set_int("kept", 7);

            declared.on = On::Monitor("placement-1".to_owned());
            declared.layer = Layer::Overlay;
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let kept = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&screen))
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
                .expect("live")
                .sync(&alone(&screen), Some(&screen));
            surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&screen))
                .expect("the scene builds")
                .scene_for_test()
                .set_int("kept", 7);

            declared.scene = second;
            assert_eq!(surfaces.declare(declared), Declared::Rebuilt);
            surfaces
                .get_mut(id)
                .expect("live")
                .sync(&alone(&screen), Some(&screen));
            let kept = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&screen))
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
            surfaces
                .get_mut(id)
                .expect("live")
                .sync(&alone(&screen), Some(&screen));
            assert!(
                surfaces
                    .get_mut(id)
                    .and_then(|surface| surface.instance_mut(&screen))
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
                .and_then(|surface| surface.instance_mut(&screen))
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
                .and_then(|surface| surface.instance_mut(&screen))
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
            let (left, right, outputs) = side_by_side("dotted-left", "dotted-right");
            let mut surfaces = Surfaces::default();
            let mut declared = Declaration::for_test("bar", path, Layer::Top, On::EveryMonitor);
            declared.properties = properties(&[
                ("count", Json::Number(4.0)),
                ("panel.open", Json::Bool(true)),
            ]);
            surfaces.declare(declared.clone());
            let id = surfaces.named("bar").expect("declared");
            surfaces
                .get_mut(id)
                .expect("live")
                .sync(&outputs[..1], Some(&left));
            let scene = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&left))
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
            surfaces
                .get_mut(id)
                .expect("live")
                .sync(&outputs, Some(&left));
            let scene = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&right))
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
                .expect("live")
                .sync(&outputs, Some(&left));
            surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&left))
                .expect("the scene builds")
                .scene_for_test()
                .set_int("kept", 7);

            declared.on = On::Monitor("moved-right".to_owned());
            declared.properties = properties(&[("label", Json::Text("two".to_owned()))]);
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs, Some(&left));
            assert!(
                surface.instance_mut(&left).is_none(),
                "the monitor it left kept its scene"
            );
            assert_eq!(surface.instance_count(), 1);
            let scene = surface
                .instance_mut(&right)
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
    /// the primary moves**, and is built on the new one: the same leak,
    /// reached by a monitor change rather than a declaration.
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
            surface.sync(&outputs, Some(&left));
            assert!(surface.instance_mut(&left).is_some(), "the scene builds");
            surface.sync(&outputs, Some(&right));
            assert!(
                surface.instance_mut(&left).is_none(),
                "the monitor that stopped being primary kept its scene"
            );
            assert!(
                surface.instance_mut(&right).is_some(),
                "the new primary has no scene"
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
            surfaces
                .get_mut(id)
                .expect("live")
                .sync(&alone(&screen), Some(&screen));
            assert!(
                surfaces
                    .get_mut(id)
                    .and_then(|surface| surface.instance_mut(&screen))
                    .is_some(),
                "the scene builds"
            );

            declared.properties = properties(&[("label", Json::Text("one".to_owned()))]);
            assert_eq!(surfaces.declare(declared), Declared::InPlace);
            let bag = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&screen))
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
            surfaces
                .get_mut(id)
                .expect("live")
                .sync(&alone(&screen), Some(&screen));
            let named = surfaces
                .get_mut(id)
                .and_then(|surface| surface.instance_mut(&screen))
                .expect("the scene builds")
                .scene_for_test()
                .get_int("named");
            assert_eq!(
                named, 1,
                "the instance's scene does not know the monitor it is on"
            );
        });
    }

    /// **A pointer event reaches the instance in the instance's own
    /// coordinates, as itself**: `Surface::deliver` is given the point in the
    /// compositor's coordinates and the area the surface is drawn across on
    /// that monitor, and the scene is told the point inside that area, with
    /// the right button as the right button (#163). Primitive 4. The scene is
    /// built 1x1 until its first frame, so the `MouseArea` has a size of its
    /// own.
    #[test]
    fn a_delivered_press_reaches_the_instance_in_its_own_coordinates() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file(
                "solium-scripted-delivered",
                "Scene.qml",
                r"
                import QtQuick
                Item {
                    property int pressedX: -1
                    property int pressedY: -1
                    property int button: 0
                    MouseArea {
                        width: 400
                        height: 30
                        acceptedButtons: Qt.AllButtons
                        onPressed: (mouse) => {
                            parent.pressedX = mouse.x
                            parent.pressedY = mouse.y
                            parent.button = mouse.button
                        }
                    }
                }
                ",
            );
            let (_, right, monitors) = side_by_side("delivered-left", "delivered-right");
            let area = Rectangle::new((1920, 0).into(), (400, 30).into());
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test(
                "bar",
                path,
                Layer::Top,
                On::Rect(area),
            ));
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&monitors, Some(&right));
            let press = ScenePointer {
                kind: PointerKind::Press(0x2),
                buttons: 0x2,
                modifiers: 0,
                time: 0,
            };
            assert!(
                surface.deliver(&right, area, (1930.0, 15.0).into(), press),
                "the instance on the right monitor did not take the press"
            );
            let scene = surface
                .instance_mut(&right)
                .expect("the scene builds")
                .scene_for_test();
            assert_eq!(
                ["pressedX", "pressedY", "button"].map(|name| scene.get_int(name)),
                [10, 15, 2],
                "[x, y, button] the scene was told"
            );
        });
    }

    /// A scene that reads whether it is on the monitor named `left`.
    fn on_left(left: &str) -> String {
        format!(
            r#"
            import QtQuick
            import Solium
            Item {{ readonly property int onLeft: Solium.monitor.name === "{left}" ? 1 : 0 }}
            "#
        )
    }

    /// **A surface on every monitor is one live scene per monitor, and each
    /// knows its own** (#161).
    #[test]
    fn a_surface_on_every_monitor_has_one_live_scene_per_monitor() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file("solium-scripted-sync", "Scene.qml", &on_left("sync-left"));
            let (left, right, outputs) = side_by_side("sync-left", "sync-right");
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test(
                "bar",
                path,
                Layer::Top,
                On::EveryMonitor,
            ));
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs, Some(&left));
            assert_eq!(surface.instance_count(), 2);
            let on = |surface: &mut super::Surface, output: &Output| {
                surface
                    .instance_mut(output)
                    .expect("an instance")
                    .scene_for_test()
                    .get_int("onLeft")
            };
            assert_eq!(
                (on(surface, &left), on(surface, &right)),
                (1, 0),
                "each instance must read its own monitor"
            );
        });
    }

    /// **Every instance's actions are taken, by monitor name**: a surface on
    /// two monitors whose scenes both send hears both, the instance on the
    /// monitor whose name sorts first first, whatever the monitors' order.
    #[test]
    fn every_instances_actions_are_taken_by_monitor_name() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file(
                "solium-scripted-take-actions",
                "Scene.qml",
                "import QtQuick\nimport Solium\nItem {\n    Component.onCompleted: Solium.send(\"hello\", Solium.monitor.name)\n}\n",
            );
            let (left, _, outputs) = side_by_side("take-actions-b", "take-actions-a");
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test(
                "bar",
                path,
                Layer::Top,
                On::EveryMonitor,
            ));
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs, Some(&left));
            let taken: Vec<(String, String)> = surface
                .take_actions()
                .into_iter()
                .map(|(action, data)| (action, data.render()))
                .collect();
            assert_eq!(
                taken,
                vec![
                    ("hello".to_owned(), r#""take-actions-a""#.to_owned()),
                    ("hello".to_owned(), r#""take-actions-b""#.to_owned()),
                ]
            );
        });
    }

    /// **An instance goes with its monitor, and a monitor that arrives gets
    /// one**, built there and then rather than at the first frame.
    #[test]
    fn an_instance_goes_with_its_monitor_and_comes_with_a_new_one() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file(
                "solium-scripted-hotplug",
                "Scene.qml",
                &on_left("hotplug-left"),
            );
            let (left, right, outputs) = side_by_side("hotplug-left", "hotplug-right");
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test(
                "bar",
                path,
                Layer::Top,
                On::EveryMonitor,
            ));
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs[..1], Some(&left));
            assert_eq!(surface.instance_count(), 1);
            surface.sync(&outputs, Some(&left));
            assert!(
                surface.instance_mut(&right).is_some(),
                "the monitor that arrived has no instance"
            );
            surface.sync(&outputs[1..], Some(&right));
            assert!(
                surface.instance_mut(&left).is_none(),
                "the monitor that went kept its instance"
            );
            assert_eq!(surface.instance_count(), 1);
        });
    }

    #[test]
    fn a_surface_on_the_primary_monitor_has_one_instance() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let path = scene_file(
                "solium-scripted-primary",
                "Scene.qml",
                &on_left("primary-one-left"),
            );
            let (_, right, outputs) = side_by_side("primary-one-left", "primary-one-right");
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test("bar", path, Layer::Top, On::Primary));
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs, Some(&right));
            assert_eq!(surface.instance_count(), 1);
            assert!(
                surface.instance_mut(&right).is_some(),
                "not on the primary monitor"
            );
        });
    }

    /// **A scene file that is not there builds nothing, until it is there**:
    /// it is not a scene that would not load, so the first sync after it
    /// appears builds it. Ruling 5.
    #[test]
    fn a_missing_scene_file_builds_nothing_until_it_is_there() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory = std::env::temp_dir().join("solium-scripted-missing");
            let _ = std::fs::remove_dir_all(&directory);
            let path = directory.join("Scene.qml");
            let (left, _, outputs) = side_by_side("missing-left", "missing-right");
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test(
                "bar",
                path,
                Layer::Top,
                On::EveryMonitor,
            ));
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs, Some(&left));
            assert_eq!(surface.instance_count(), 0);

            scene_file("solium-scripted-missing", "Scene.qml", KEPT);
            surface.sync(&outputs, Some(&left));
            assert_eq!(
                surface.instance_count(),
                2,
                "the scene that appeared was taken for one that would not load"
            );
        });
    }

    /// **A surface claims nothing on a monitor it has no instance on**: with
    /// its scene file missing none is built, and a point inside the area it
    /// would be drawn across is nobody's, so what is under it keeps it.
    #[test]
    fn a_surface_with_no_instance_on_a_monitor_claims_nothing_there() {
        on_the_qt_thread(|| {
            let (left, _, outputs) = side_by_side("no-instance-left", "no-instance-right");
            let mut surfaces = Surfaces::default();
            surfaces.declare(Declaration::for_test(
                "bar",
                PathBuf::from("/nonexistent/solium-no-instance.qml"),
                Layer::Top,
                On::EveryMonitor,
            ));
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs, Some(&left));
            let area = Rectangle::new((0, 0).into(), (1920, 1080).into());
            assert_eq!(
                (
                    surface.instance_count(),
                    surface.hit(&left, area, (10.0, 10.0).into())
                ),
                (0, Hit::Nothing),
                "(the instances built, what the surface claims inside its area)"
            );
        });
    }

    /// **A scene that will not load is not tried again on the same monitor
    /// until the surface names another scene file**, so a broken scene is a
    /// line in the log per monitor rather than a build on every sync. Ruling 5.
    #[test]
    fn a_scene_that_will_not_load_waits_for_another_scene_file() {
        on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let broken = scene_file(
                "solium-scripted-broken",
                "Broken.qml",
                "import QtQuick\nItem { nonsense: }\n",
            );
            let (left, _, outputs) = side_by_side("broken-left", "broken-right");
            let mut surfaces = Surfaces::default();
            let mut declared =
                Declaration::for_test("bar", broken.clone(), Layer::Top, On::EveryMonitor);
            surfaces.declare(declared.clone());
            let id = surfaces.named("bar").expect("declared");
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs, Some(&left));
            assert_eq!(surface.instance_count(), 0, "a broken scene built");

            // Mended, and Qt's cache of the failure forgotten as a reload
            // forgets it, so only the surface itself can say not to retry.
            std::fs::write(&broken, KEPT).expect("mending the scene");
            crate::qml::clear_cache();
            surface.sync(&outputs, Some(&left));
            assert_eq!(
                surface.instance_count(),
                0,
                "a scene that would not load was tried again"
            );

            declared.scene = scene_file("solium-scripted-mended", "Scene.qml", KEPT);
            assert_eq!(surfaces.declare(declared), Declared::Rebuilt);
            let surface = surfaces.get_mut(id).expect("live");
            surface.sync(&outputs, Some(&left));
            assert_eq!(
                surface.instance_count(),
                2,
                "another scene file was not built"
            );
        });
    }

    /// **Off the Qt thread nothing is built**, whatever the scene: a test that
    /// is not on it, as no test driving a real Wayland client is, would
    /// otherwise build there the shipped wallpaper its configuration declares,
    /// and Qt answers a scene built off its thread with a `qFatal` that takes
    /// the whole test binary down (the #99 rule).
    #[test]
    fn a_surface_synced_off_the_qt_thread_builds_nothing() {
        let path = scene_file("solium-scripted-off-thread", "Scene.qml", KEPT);
        let (left, _, outputs) = side_by_side("off-thread-left", "off-thread-right");
        let mut surfaces = Surfaces::default();
        surfaces.declare(Declaration::for_test(
            "bar",
            path,
            Layer::Top,
            On::EveryMonitor,
        ));
        let id = surfaces.named("bar").expect("declared");
        let surface = surfaces.get_mut(id).expect("live");
        surface.sync(&outputs, Some(&left));
        assert_eq!(
            surface.instance_count(),
            0,
            "a scene was built off the Qt thread"
        );
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
