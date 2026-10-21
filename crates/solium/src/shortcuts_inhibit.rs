//! `zwp_keyboard_shortcuts_inhibit_manager_v1`: a window that is itself a
//! desktop -- a virtual machine, a remote desktop, a nested compositor --
//! asks for the compositor's own key combinations, and gets them (#58).
//!
//! Without it every combination Solium binds is taken before the client sees
//! it, and `super` never reaches the machine on the other end.
//!
//! ## The rules
//!
//! - **Only the window with the keyboard.** An inhibitor is active while its
//!   surface has keyboard focus and at no other time: a window that asked and
//!   went to the background must not keep the shortcuts. It is made active
//!   again when the keyboard comes back. [`Solium::settle_inhibitors`], from
//!   `SeatHandler::focus_changed`.
//! - **The escapes stay.** `Ctrl+Alt+F*` and `Ctrl+Alt+Backspace` are tried
//!   before anything else in `input::press`, and an inhibitor is asked after
//!   them, so a client that has every other key still cannot trap the session.
//!   Nor does one reach past the lock, or past a shell scene or a script mode
//!   holding the keyboard: those are asked first too.
//! - **A binding that asked with `inhibited` still fires**, and nothing else
//!   does. The shipped one is `super+Escape`, bound in `init.lua` to
//!   `sol.take_shortcuts_back`: with the shortcuts given away there is
//!   otherwise no key that moves the keyboard to another window. Taken back,
//!   an inhibitor is inactive until the keyboard leaves its window, and the
//!   next time the window gets the keyboard it holds the shortcuts again.
//!
//! Which applications may ask (always, fullscreen only, per application), an
//! indicator for the bar and XWayland's keyboard grab are #245.

use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};

use crate::state::Solium;

/// What is kept of the inhibitors clients have made.
#[derive(Debug)]
pub(crate) struct Inhibitors {
    /// Registers the global; smithay keeps the rest.
    state: KeyboardShortcutsInhibitState,
    /// Every inhibitor a client holds, active or not. Smithay keeps its own
    /// list per seat and offers no way to walk it, and the focus changing
    /// has to visit all of them.
    held: Vec<KeyboardShortcutsInhibitor>,
    /// The surface whose shortcuts `sol.take_shortcuts_back` took back, until the
    /// keyboard next leaves it.
    taken_back: Option<WlSurface>,
}

impl Inhibitors {
    pub(crate) fn new(display: &smithay::reexports::wayland_server::DisplayHandle) -> Self {
        Self {
            state: KeyboardShortcutsInhibitState::new::<Solium>(display),
            held: Vec::new(),
            taken_back: None,
        }
    }

    /// Whether the window with the keyboard holds the shortcuts now.
    pub(crate) fn active(&self) -> bool {
        self.held.iter().any(KeyboardShortcutsInhibitor::is_active)
    }

    /// Make each inhibitor active exactly when its surface is `focus` and its
    /// shortcuts have not been taken back, telling each client only when that
    /// changes.
    fn settle(&mut self, focus: Option<&WlSurface>) {
        if self.taken_back.as_ref() != focus {
            self.taken_back = None;
        }
        self.held
            .retain(|inhibitor| inhibitor.wl_surface().is_alive());
        for inhibitor in &self.held {
            let surface = inhibitor.wl_surface();
            let wanted = focus == Some(surface) && self.taken_back.as_ref() != Some(surface);
            if wanted != inhibitor.is_active() {
                if wanted {
                    inhibitor.activate();
                } else {
                    inhibitor.inactivate();
                }
                tracing::debug!(active = wanted, "keyboard shortcuts inhibitor");
            }
        }
    }
}

impl Solium {
    /// The surface the keyboard is on, as smithay's keyboard has it.
    fn keyboard_surface(&self) -> Option<WlSurface> {
        self.seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
    }

    /// The keyboard moved to `focus`: the inhibitor on it, if any, becomes
    /// active, and every other one inactive.
    pub(crate) fn settle_inhibitors(&mut self, focus: Option<&WlSurface>) {
        self.inhibitors.settle(focus);
    }

    /// `sol.take_shortcuts_back`, from a binding that asked with `inhibited`:
    /// take them back from the window with the keyboard until the keyboard
    /// leaves it.
    pub(crate) fn take_shortcuts_back(&mut self) {
        let focus = self.keyboard_surface();
        self.inhibitors.taken_back.clone_from(&focus);
        self.inhibitors.settle(focus.as_ref());
        tracing::info!("keyboard shortcuts taken back from the focused window");
    }
}

impl KeyboardShortcutsInhibitHandler for Solium {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.inhibitors.state
    }

    /// Kept, and active at once if its surface already has the keyboard,
    /// which is the ordinary case: a VM viewer asks once its window is
    /// focused.
    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        self.inhibitors.held.push(inhibitor);
        let focus = self.keyboard_surface();
        self.inhibitors.settle(focus.as_ref());
    }

    fn inhibitor_destroyed(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        self.inhibitors
            .held
            .retain(|held| held.wl_surface() != inhibitor.wl_surface());
    }
}

smithay::delegate_keyboard_shortcuts_inhibit!(Solium);
