//! The windows, for a shell that is its own program (#50): a dock, a task
//! switcher, a panel's window list.
//!
//! Two protocols, both views of one list, `Solium::window_infos`, the same
//! windows a layout and `sol.windows()` see:
//!
//! * `ext-foreign-toplevel-list-v1`: each window's identity, title and
//!   `app_id`, and when it goes. Smithay's.
//! * `zwlr-foreign-toplevel-management-v1`: the same, with the monitor a
//!   window is on and whether it is maximised, minimised, focused or
//!   fullscreen, and requests to change each of those or close it. Written
//!   here, since Smithay has none.
//!
//! **What a client asks is an action, and the configuration answers it.** A
//! request reaches Lua as an action of the `windows` vocabulary from the
//! surface named [`SURFACE`] -- `windows.focus`, `windows.close` and the
//! rest, exactly what Solium's own taskbar sends -- so `actions.lua` routes
//! it, an override of the configuration's answers it, and a dock that is a
//! client does what a dock that is a scene does. Nothing here focuses or
//! closes a window itself.
//!
//! Every window a script is told of is listed, and nothing else: a menu, a
//! window whose application has not arrived, one that is leaving. Both are
//! offered to every client; `toplevels.list` and `toplevels.manage` in
//! `config.lua` take either away.
//!
//! A window on the ext list is also what a screen share names to share one
//! window rather than a monitor (#47): `image_copy.rs` reads the window back
//! from its handle ([`listed_id`]), and `toplevels.capture` says what of it
//! is in the picture.
//! `state::tests::real_client::reflow_on_close::toplevels`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
            zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, Rectangle},
    wayland::foreign_toplevel_list::{
        ForeignToplevelHandle, ForeignToplevelListHandler, ForeignToplevelListState,
    },
};

use crate::json::Json;
use crate::state::Solium;

/// The surface name a client's request reaches Lua's `surface` listeners
/// with: `sol.on("surface", function(surface, action, data) end)` hears
/// `surface == "foreign-toplevel"`.
pub(crate) const SURFACE: &str = "foreign-toplevel";

/// The version of `zwlr_foreign_toplevel_manager_v1` offered: 3, with
/// fullscreen (2) and a window's parent (3).
const WLR_VERSION: u32 = 3;

/// `config.toplevels`, handed over by `sol.toplevels`.
/// `script::tests::sol_toplevels_reads_each_protocol_as_a_flag`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Whether `ext_foreign_toplevel_list_v1` is offered.
    pub(crate) list: bool,
    /// Whether `zwlr_foreign_toplevel_manager_v1` is offered.
    pub(crate) manage: bool,
    /// What a capture of one window holds, or whether a window can be
    /// captured at all (#47).
    pub(crate) capture: WindowCapture,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            list: true,
            manage: true,
            capture: WindowCapture::Client,
        }
    }
}

/// `toplevels.capture`: what a capture of one window is a picture of, through
/// `ext_foreign_toplevel_image_capture_source_manager_v1`.
/// `state::tests::real_client::reflow_on_close::window_capture`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowCapture {
    /// No window is offered as a capture source; monitors still are.
    Off,
    /// The window as its application drew it, its popups over it: the same
    /// picture whatever frame the desktop puts around it. `"client"`.
    Client,
    /// The window as it is on screen, the frame and the effects the
    /// configuration draws on it included. `"pane"`.
    Pane,
}

impl WindowCapture {
    /// Read from its Lua spelling: `"client"`, `"pane"`, or `false`.
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "client" => Some(Self::Client),
            "pane" => Some(Self::Pane),
            _ => None,
        }
    }
}

/// The window an ext list handle was made for, kept in the handle's own
/// data: what a capture source made from it names.
#[derive(Debug)]
struct ListedId(u64);

/// The window `handle` was listed for, by the id scripts hold it by.
pub(crate) fn listed_id(
    handle: &smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::server::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
) -> Option<u64> {
    ForeignToplevelHandle::from_resource(handle)?
        .user_data()
        .get::<ListedId>()
        .map(|id| id.0)
}

/// What a client is told of one window, compared with what it was last told
/// so each change is sent once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Facts {
    title: String,
    app_id: String,
    monitor: String,
    maximized: bool,
    minimized: bool,
    activated: bool,
    fullscreen: bool,
    parent: Option<u64>,
}

impl Facts {
    /// The `state` array the wlr protocol sends: each state a little-endian
    /// `u32`.
    fn states(&self) -> Vec<u8> {
        use zwlr_foreign_toplevel_handle_v1::State;
        [
            (self.maximized, State::Maximized),
            (self.minimized, State::Minimized),
            (self.activated, State::Activated),
            (self.fullscreen, State::Fullscreen),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .flat_map(|(_, state)| u32::from(state).to_ne_bytes())
        .collect()
    }
}

/// One listed window.
#[derive(Debug)]
struct Listed {
    /// Its `ext_foreign_toplevel_handle_v1`, when the list is offered.
    ext: Option<ForeignToplevelHandle>,
    /// Its `zwlr_foreign_toplevel_handle_v1`, one per manager bound.
    wlr: Vec<ZwlrForeignToplevelHandleV1>,
    facts: Facts,
}

/// The data a wlr handle carries: the window's id, as scripts hold it.
#[derive(Debug)]
pub(crate) struct HandleData(u64);

/// Whether a global is offered: read by its `can_view`, so a global taken
/// away is hidden from every client that has not bound it, and nothing a
/// client already holding it sends can bring it back.
#[derive(Debug, Clone)]
pub(crate) struct Offered(Arc<AtomicBool>);

impl Offered {
    fn on(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Both protocols' state.
#[derive(Debug)]
pub(crate) struct Toplevels {
    pub(crate) settings: Settings,
    list: ForeignToplevelListState,
    list_offered: Offered,
    manage_offered: Offered,
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    listed: BTreeMap<u64, Listed>,
    /// What clients asked, for [`Solium::settle_toplevel_requests`] to hand
    /// to Lua.
    asked: Vec<(String, Json)>,
    /// Where each window's dock icon is, as a client said with
    /// `set_rectangle`: by window, the surface's rectangle in the
    /// compositor's space. `sol.windows()` gives it as `icon`.
    icons: BTreeMap<u64, Rectangle<i32, Logical>>,
}

impl Toplevels {
    /// Both offered, as the default configuration does.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let defaults = Settings::default();
        let list_offered = Offered(Arc::new(AtomicBool::new(defaults.list)));
        let manage_offered = Offered(Arc::new(AtomicBool::new(defaults.manage)));
        let seen = list_offered.clone();
        let list = ForeignToplevelListState::new_with_filter::<Solium>(display, move |_| seen.on());
        display.create_global::<Solium, ZwlrForeignToplevelManagerV1, _>(
            WLR_VERSION,
            manage_offered.clone(),
        );
        Self {
            settings: defaults,
            list,
            list_offered,
            manage_offered,
            managers: Vec::new(),
            listed: BTreeMap::new(),
            asked: Vec::new(),
            icons: BTreeMap::new(),
        }
    }

    /// Offer each protocol, or take it away, as `settings` says. Taken away,
    /// it is hidden from clients that have not bound it, and every handle
    /// given out through it is closed, so no client keeps acting on windows
    /// it was told were gone. Offered again, the windows are listed again.
    pub(crate) fn configure(&mut self, settings: Settings) {
        self.list_offered.0.store(settings.list, Ordering::Relaxed);
        self.manage_offered
            .0
            .store(settings.manage, Ordering::Relaxed);
        if !settings.list {
            for listed in self.listed.values_mut() {
                if let Some(handle) = listed.ext.take() {
                    self.list.remove_toplevel(&handle);
                }
            }
        }
        if !settings.manage {
            for listed in self.listed.values_mut() {
                for handle in listed.wlr.drain(..) {
                    handle.closed();
                }
            }
            for manager in self.managers.drain(..) {
                manager.finished();
            }
        }
        self.settings = settings;
    }

    /// Where a client said a window's icon is, if one did.
    pub(crate) fn icon(&self, id: u64) -> Option<Rectangle<i32, Logical>> {
        self.icons.get(&id).copied()
    }

    /// What clients asked since the last call, oldest first.
    pub(crate) fn take_asked(&mut self) -> Vec<(String, Json)> {
        std::mem::take(&mut self.asked)
    }
}

/// A window's facts now, by id: the windows a script is told of, and what
/// each client of either protocol may know of it.
fn facts_now(state: &Solium) -> BTreeMap<u64, Facts> {
    state
        .window_infos()
        .into_iter()
        .filter(|info| !info.leaving)
        .filter_map(|info| {
            let pane = state.panes.by_script_id(info.id)?;
            let window = pane.client()?;
            let states = crate::state::window_states(window);
            Some((
                info.id,
                Facts {
                    title: info.title.clone(),
                    app_id: info.app_id.clone(),
                    monitor: info.monitor.clone(),
                    maximized: states.maximized,
                    minimized: info.minimized,
                    activated: info.focused,
                    fullscreen: states.fullscreen,
                    parent: match info.parent {
                        crate::script::Parentage::Window(parent) => Some(parent),
                        crate::script::Parentage::None | crate::script::Parentage::Unknown => None,
                    },
                },
            ))
        })
        .collect()
}

impl Solium {
    /// Tell both protocols' clients what changed among the windows since
    /// they were last told: a window that opened, one that went, and each
    /// fact of a window that changed, each followed by its `done`. Every
    /// pass, beside the models, and as cheap when nothing changed: one walk
    /// of the windows and a comparison each.
    pub(crate) fn publish_toplevels(&mut self) {
        self.follow_window_captures();
        if !self.toplevels.settings.list && self.toplevels.managers.is_empty() {
            self.toplevels.listed.clear();
            return;
        }
        let now = facts_now(self);
        let gone: Vec<u64> = self
            .toplevels
            .listed
            .keys()
            .filter(|id| !now.contains_key(id))
            .copied()
            .collect();
        for id in gone {
            if let Some(listed) = self.toplevels.listed.remove(&id) {
                if let Some(handle) = listed.ext {
                    self.toplevels.list.remove_toplevel(&handle);
                }
                for handle in listed.wlr {
                    handle.closed();
                }
            }
            self.toplevels.icons.remove(&id);
        }
        for (id, facts) in &now {
            // Listed on the ext list whenever it is offered: a window that
            // opened while it was taken away is listed once it is back.
            let unlisted = self
                .toplevels
                .listed
                .get(id)
                .is_none_or(|listed| listed.ext.is_none());
            if self.toplevels.settings.list && unlisted {
                let handle = self
                    .toplevels
                    .list
                    .new_toplevel::<Self>(&facts.title, &facts.app_id);
                let listed = *id;
                handle
                    .user_data()
                    .insert_if_missing_threadsafe(|| ListedId(listed));
                handle.send_done();
                if let Some(listed) = self.toplevels.listed.get_mut(id) {
                    listed.ext = Some(handle);
                    // Told every fact it has now; the wlr handles below still
                    // hear what changed.
                } else {
                    self.toplevels.listed.insert(
                        *id,
                        Listed {
                            ext: Some(handle),
                            wlr: Vec::new(),
                            facts: Facts::default(),
                        },
                    );
                    let managers = self.toplevels.managers.clone();
                    for manager in &managers {
                        self.announce(manager, *id);
                    }
                    if let Some(listed) = self.toplevels.listed.get_mut(id) {
                        listed.facts = facts.clone();
                    }
                    continue;
                }
            }
            if !self.toplevels.listed.contains_key(id) {
                self.toplevels.listed.insert(
                    *id,
                    Listed {
                        ext: None,
                        wlr: Vec::new(),
                        facts: Facts::default(),
                    },
                );
                let managers = self.toplevels.managers.clone();
                for manager in &managers {
                    self.announce(manager, *id);
                }
                // `announce` sent every fact the window had; it is told only
                // what changes from here on.
                if let Some(listed) = self.toplevels.listed.get_mut(id) {
                    listed.facts = facts.clone();
                }
                continue;
            }
            let was = self
                .toplevels
                .listed
                .get(id)
                .map(|listed| listed.facts.clone())
                .unwrap_or_default();
            if was == *facts {
                continue;
            }
            let outputs = self.outputs_named(&[&was.monitor, &facts.monitor]);
            let parent = facts.parent.and_then(|parent| {
                self.toplevels
                    .listed
                    .get(&parent)
                    .map(|listed| listed.wlr.clone())
            });
            let Some(listed) = self.toplevels.listed.get_mut(id) else {
                continue;
            };
            if let Some(handle) = &listed.ext
                && !unlisted
                && (was.title != facts.title || was.app_id != facts.app_id)
            {
                if was.title != facts.title {
                    handle.send_title(&facts.title);
                }
                if was.app_id != facts.app_id {
                    handle.send_app_id(&facts.app_id);
                }
                handle.send_done();
            }
            for handle in &listed.wlr {
                tell(handle, Some(&was), facts, &outputs, parent.as_deref());
            }
            listed.facts = facts.clone();
        }
    }

    /// A new wlr handle for window `id`, given to `manager`'s client and told
    /// everything the window is.
    fn announce(&mut self, manager: &ZwlrForeignToplevelManagerV1, id: u64) {
        let Some(client) = manager.client() else {
            return;
        };
        let Ok(handle) = client.create_resource::<ZwlrForeignToplevelHandleV1, _, Self>(
            &self.display_handle,
            manager.version(),
            HandleData(id),
        ) else {
            return;
        };
        manager.toplevel(&handle);
        let facts = facts_now(self).remove(&id).unwrap_or_default();
        let outputs = self.outputs_named(&[&facts.monitor]);
        let parent = facts.parent.and_then(|parent| {
            self.toplevels
                .listed
                .get(&parent)
                .map(|listed| listed.wlr.clone())
        });
        tell(&handle, None, &facts, &outputs, parent.as_deref());
        if let Some(listed) = self.toplevels.listed.get_mut(&id) {
            listed.wlr.push(handle);
        }
    }

    /// The monitors these names are, by name.
    fn outputs_named(&self, names: &[&str]) -> BTreeMap<String, Output> {
        self.space
            .outputs()
            .filter(|output| names.contains(&output.name().as_str()))
            .map(|output| (output.name(), output.clone()))
            .collect()
    }

    /// Where a client's surface is in the compositor's space: a window's, or
    /// a layer surface's, which is what `set_rectangle` measures from.
    fn surface_origin(&self, surface: &WlSurface) -> Option<Point<i32, Logical>> {
        if let Some(window) = self.window_for(surface) {
            return self.space.element_location(&window);
        }
        self.space.outputs().find_map(|output| {
            let map = smithay::desktop::layer_map_for_output(output);
            let layer = map.layer_for_surface(surface, smithay::desktop::WindowSurfaceType::ALL)?;
            let geometry = map.layer_geometry(layer)?;
            let monitor = self.space.output_geometry(output)?;
            Some(monitor.loc + geometry.loc)
        })
    }

    /// Hand what clients asked to Lua, each as an action of the surface
    /// [`SURFACE`], each in a dispatch of its own, as a scene's actions are
    /// (`Solium::settle_actions`).
    pub(crate) fn settle_toplevel_requests(&mut self) {
        for (action, data) in self.toplevels.take_asked() {
            let snapshot = self.snapshot();
            let Some(mut scripts) = self.scripts.take() else {
                return;
            };
            let outcome = scripts.surface_action(SURFACE, &action, &data, snapshot);
            self.scripts = Some(scripts);
            self.apply(outcome);
        }
    }
}

/// Tell one wlr handle what changed from `was` to `now` -- everything, for a
/// handle that was told nothing -- and `done`. Its monitors are its own
/// client's `wl_output`s for each.
fn tell(
    handle: &ZwlrForeignToplevelHandleV1,
    was: Option<&Facts>,
    now: &Facts,
    outputs: &BTreeMap<String, Output>,
    parent: Option<&[ZwlrForeignToplevelHandleV1]>,
) {
    let title = was.is_none_or(|was| was.title != now.title);
    let app_id = was.is_none_or(|was| was.app_id != now.app_id);
    let monitor = was.is_none_or(|was| was.monitor != now.monitor);
    let states = was.is_none_or(|was| was.states() != now.states());
    let parentage = was.is_none_or(|was| was.parent != now.parent);
    if title {
        handle.title(now.title.clone());
    }
    if app_id {
        handle.app_id(now.app_id.clone());
    }
    if monitor && let Some(client) = handle.client() {
        if let Some(was) = was
            && let Some(output) = outputs.get(&was.monitor)
        {
            for each in output.client_outputs(&client) {
                handle.output_leave(&each);
            }
        }
        if let Some(output) = outputs.get(&now.monitor) {
            for each in output.client_outputs(&client) {
                handle.output_enter(&each);
            }
        }
    }
    if states {
        handle.state(now.states());
    }
    if parentage && handle.version() >= 3 {
        // The parent's handle on this same client's manager, if it has one.
        let mine = parent.and_then(|handles| {
            handles
                .iter()
                .find(|each| each.client() == handle.client())
                .cloned()
        });
        handle.parent(mine.as_ref());
    }
    handle.done();
}

impl ForeignToplevelListHandler for Solium {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.toplevels.list
    }
}

smithay::delegate_foreign_toplevel_list!(Solium);

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, Offered> for Solium {
    fn bind(
        state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        offered: &Offered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        // Bound just as it was taken away: told it is over, and nothing else.
        if !offered.on() {
            manager.finished();
            return;
        }
        // Every window there is now, then each as it comes.
        let ids: Vec<u64> = state.toplevels.listed.keys().copied().collect();
        for id in ids {
            state.announce(&manager, id);
        }
        state.toplevels.managers.push(manager);
    }

    fn can_view(_client: Client, offered: &Offered) -> bool {
        offered.on()
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            state.toplevels.managers.retain(|each| each != manager);
            manager.finished();
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        manager: &ZwlrForeignToplevelManagerV1,
        (): &(),
    ) {
        state.toplevels.managers.retain(|each| each != manager);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, HandleData> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        handle: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        data: &HandleData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request;
        let id = data.0;
        let facts = state.toplevels.listed.get(&id).map(|listed| &listed.facts);
        let Some(facts) = facts.cloned() else {
            // A window that went: its handle was told `closed`, and a request
            // already on its way is nobody's.
            return;
        };
        // The vocabulary's toggles, sent only when they change something: a
        // client asking to maximise a maximised window asks for nothing.
        let action = match request {
            Request::SetMaximized => (!facts.maximized).then_some("windows.maximize"),
            Request::UnsetMaximized => facts.maximized.then_some("windows.maximize"),
            Request::SetMinimized => (!facts.minimized).then_some("windows.minimize"),
            Request::UnsetMinimized => facts.minimized.then_some("windows.unminimize"),
            Request::SetFullscreen { .. } => (!facts.fullscreen).then_some("windows.fullscreen"),
            Request::UnsetFullscreen => facts.fullscreen.then_some("windows.fullscreen"),
            Request::Activate { .. } => Some("windows.focus"),
            Request::Close => Some("windows.close"),
            Request::SetRectangle {
                surface,
                x,
                y,
                width,
                height,
            } => {
                if width < 0 || height < 0 {
                    handle.post_error(
                        zwlr_foreign_toplevel_handle_v1::Error::InvalidRectangle,
                        "a rectangle's size is never negative",
                    );
                    return;
                }
                // Relative to the surface, which is a layer surface or a
                // window of the client's; nothing at all clears it.
                let origin = state.surface_origin(&surface);
                match origin {
                    Some(origin) if width > 0 && height > 0 => {
                        state.toplevels.icons.insert(
                            id,
                            Rectangle::new(
                                (origin.x + x, origin.y + y).into(),
                                (width, height).into(),
                            ),
                        );
                    }
                    _ => {
                        state.toplevels.icons.remove(&id);
                    }
                }
                None
            }
            Request::Destroy => None,
            _ => None,
        };
        if let Some(action) = action {
            let mut data = BTreeMap::new();
            #[expect(clippy::cast_precision_loss, reason = "an id, far below 2^53")]
            data.insert("id".to_owned(), Json::Number(id as f64));
            state
                .toplevels
                .asked
                .push((action.to_owned(), Json::Object(data)));
            // Answered now, as a scene's action is at its release: a dock
            // that is told nothing until the next frame is a dock that feels
            // late.
            state.settle_toplevel_requests();
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        handle: &ZwlrForeignToplevelHandleV1,
        data: &HandleData,
    ) {
        if let Some(listed) = state.toplevels.listed.get_mut(&data.0) {
            listed.wlr.retain(|each| each != handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_array_holds_each_state_that_is_on_as_a_native_u32() {
        use zwlr_foreign_toplevel_handle_v1::State;
        let facts = Facts {
            minimized: true,
            fullscreen: true,
            ..Facts::default()
        };
        let expected: Vec<u8> = [State::Minimized, State::Fullscreen]
            .into_iter()
            .flat_map(|state| u32::from(state).to_ne_bytes())
            .collect();
        assert_eq!(facts.states(), expected);
        assert!(Facts::default().states().is_empty());
    }
}
