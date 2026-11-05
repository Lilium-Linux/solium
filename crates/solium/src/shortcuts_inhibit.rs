//! Focus-scoped shortcut requests from Wayland and XWayland clients.
//!
//! Policy is configured by `sol.input`. Both protocols share eligibility,
//! reclaim and published ownership. XWayland's request is deliberately not a
//! Smithay keyboard grab: it may take shortcuts while focused, but cannot pin
//! focus over a user action, a hosted scene or the lock. The protocol permits
//! selective delivery and rerouting while its resource is alive.

use smithay::input::Seat;
use smithay::reexports::wayland_protocols::xwayland::keyboard_grab::zv1::server::zwp_xwayland_keyboard_grab_v1::ZwpXwaylandKeyboardGrabV1;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};
use smithay::wayland::xwayland_keyboard_grab::{
    XWaylandKeyboardGrab, XWaylandKeyboardGrabHandler, XWaylandKeyboardGrabState,
};

use crate::effect::rules::Glob;
use crate::state::Solium;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Allow {
    #[default]
    Always,
    Never,
    Fullscreen,
}

#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pub(crate) allow: Allow,
    pub(crate) escape: String,
    /// Exact names or trailing-star prefixes, matching native app_id or WM_CLASS.
    pub(crate) deny: Vec<Glob>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            allow: Allow::Always,
            escape: "super+escape".into(),
            deny: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Inhibitors {
    state: KeyboardShortcutsInhibitState,
    /// Owns the restricted global for the life of the compositor.
    _xwayland: XWaylandKeyboardGrabState,
    held: Vec<KeyboardShortcutsInhibitor>,
    xwayland: Vec<(WlSurface, ZwpXwaylandKeyboardGrabV1)>,
    /// Reclaimed until keyboard focus leaves this surface. Policy changes
    /// and request replacement do not undo an explicit reclaim.
    taken_back: Option<WlSurface>,
    pub(crate) settings: Settings,
    owner: Option<(WlSurface, u64)>,
}

impl Inhibitors {
    pub(crate) fn new(display: &smithay::reexports::wayland_server::DisplayHandle) -> Self {
        Self {
            state: KeyboardShortcutsInhibitState::new::<Solium>(display),
            _xwayland: XWaylandKeyboardGrabState::new::<Solium>(display),
            held: Vec::new(),
            xwayland: Vec::new(),
            taken_back: None,
            settings: Settings::default(),
            owner: None,
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.owner.as_ref().is_some_and(|(surface, _)| {
            surface.is_alive()
                && (self
                    .held
                    .iter()
                    .any(|held| held.wl_surface() == surface && held.is_active())
                    || self
                        .xwayland
                        .iter()
                        .any(|(held, resource)| held == surface && resource.is_alive()))
        })
    }

    pub(crate) fn owner(&self) -> Option<u64> {
        self.owner
            .as_ref()
            .filter(|_| self.active())
            .map(|(_, id)| *id)
    }

    fn settle(&mut self, focus: Option<&WlSurface>, allowed: Option<u64>) -> bool {
        let before = self.owner.clone();
        if self.taken_back.as_ref() != focus {
            self.taken_back = None;
        }
        self.held
            .retain(|inhibitor| inhibitor.wl_surface().is_alive());
        self.xwayland
            .retain(|(surface, resource)| surface.is_alive() && resource.is_alive());
        let eligible = allowed.is_some() && self.taken_back.as_ref() != focus;
        for inhibitor in &self.held {
            let wanted = eligible && focus == Some(inhibitor.wl_surface());
            if wanted != inhibitor.is_active() {
                if wanted {
                    inhibitor.activate();
                } else {
                    inhibitor.inactivate();
                }
            }
        }
        self.owner = focus
            .zip(allowed)
            .filter(|(surface, _)| {
                eligible
                    && (self
                        .held
                        .iter()
                        .any(|held| held.wl_surface() == *surface && held.is_active())
                        || self.xwayland.iter().any(|(held, _)| held == *surface))
            })
            .map(|(surface, id)| (surface.clone(), id));
        before != self.owner
    }
}

impl Solium {
    fn keyboard_surface(&self) -> Option<WlSurface> {
        self.seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
    }

    /// Also runs outside focus callbacks, for policy, fullscreen, identity
    /// and resource lifetime changes. Never call this inside the seat filter.
    pub(crate) fn refresh_inhibitors(&mut self) {
        if self.inhibitors.held.is_empty()
            && self.inhibitors.xwayland.is_empty()
            && self.inhibitors.owner.is_none()
            && self.inhibitors.taken_back.is_none()
        {
            return;
        }
        let focus = self.keyboard_surface();
        self.settle_inhibitors(focus.as_ref());
    }

    /// The supplied focus is used directly: a SeatHandler callback can run
    /// while the keyboard is locked, so querying it again here deadlocks.
    pub(crate) fn settle_inhibitors(&mut self, focus: Option<&WlSurface>) {
        let allowed = focus
            .filter(|_| {
                self.lock.is_none()
                    && !self.script_grab
                    && self.hosted_keyboard.is_none()
                    && self.decorations.holding.is_none()
            })
            .and_then(|surface| self.window_for(surface))
            .filter(|window| {
                let settings = &self.inhibitors.settings;
                let app = self.script_app_id(window);
                let fullscreen = crate::state::window_states(window).fullscreen;
                settings.allow != Allow::Never
                    && (settings.allow != Allow::Fullscreen || fullscreen)
                    && !settings.deny.iter().any(|pattern| pattern.matches(&app))
            })
            .map(|window| self.window_id(&window));
        if self.inhibitors.settle(focus, allowed) {
            self.redraw = true;
        }
    }

    /// One source for Lua and QML ownership, including lock privacy.
    pub(crate) fn shortcuts_owner(&self) -> Option<u64> {
        self.inhibitors.owner().filter(|_| self.lock.is_none())
    }

    pub(crate) fn configure_inhibitors(&mut self, settings: Settings) {
        self.inhibitors.settings = settings;
        self.refresh_inhibitors();
    }

    pub(crate) fn take_shortcuts_back(&mut self) {
        self.inhibitors.taken_back = self.keyboard_surface();
        self.refresh_inhibitors();
        tracing::info!("keyboard shortcuts taken back from the focused window");
    }
}

impl KeyboardShortcutsInhibitHandler for Solium {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.inhibitors.state
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        self.inhibitors.held.push(inhibitor);
        self.refresh_inhibitors();
    }

    fn inhibitor_destroyed(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        self.inhibitors
            .held
            .retain(|held| held.wl_surface() != inhibitor.wl_surface());
        self.refresh_inhibitors();
    }
}

impl XWaylandKeyboardGrabHandler for Solium {
    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<WlSurface> {
        self.window_for(surface)
            .filter(|window| window.x11_surface().is_some())
            .map(|_| surface.clone())
    }

    fn grab(&mut self, surface: WlSurface, seat: Seat<Self>, grab: XWaylandKeyboardGrab<Self>) {
        if seat != self.seat || self.keyboard_focus_for_xsurface(&surface).is_none() {
            return;
        }
        self.inhibitors
            .xwayland
            .push((surface, grab.grab().clone()));
        self.refresh_inhibitors();
    }
}

smithay::delegate_keyboard_shortcuts_inhibit!(Solium);
smithay::delegate_xwayland_keyboard_grab!(Solium);
