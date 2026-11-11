//! A window manager's copy of the desktop, kept by the changes each batch
//! carries, so that every question it asks of the desktop is answered here
//! and never crosses the socket.

use std::collections::HashMap;

use crate::{Change, Doc, Keyboard, Monitor, Problem, Rect, TextField, Window};

/// The desktop as the last batch left it.
#[derive(Clone, Debug, Default)]
pub struct Mirror {
    windows: HashMap<u64, Window>,
    /// Window ids, top to bottom.
    stacking: Vec<u64>,
    pub monitors: Vec<Monitor>,
    pub work_area: Option<Rect>,
    pub cursor: (f64, f64),
    pub keyboard: Option<Keyboard>,
    pub shortcuts_owner: Option<u64>,
    pub text_field: Option<TextField>,
    pub problems: Vec<Problem>,
    pub locked: bool,
    /// Each session service's view, and the installed applications', by
    /// name.
    pub services: HashMap<String, Doc>,
}

impl Mirror {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one change.
    pub fn apply(&mut self, change: Change) {
        match change {
            Change::Window(window) => {
                if !self.windows.contains_key(&window.id) {
                    // A new window is on top until a stacking says where.
                    self.stacking.insert(0, window.id);
                }
                self.windows.insert(window.id, window);
            }
            Change::WindowGone(id) => {
                self.windows.remove(&id);
                self.stacking.retain(|each| *each != id);
            }
            Change::Stacking(order) => self.stacking = order,
            Change::Monitors(monitors) => self.monitors = monitors,
            Change::WorkArea(area) => self.work_area = Some(area),
            Change::Cursor(at) => self.cursor = (at.x, at.y),
            Change::Keyboard(keyboard) => self.keyboard = Some(keyboard),
            Change::ShortcutsOwner(owner) => self.shortcuts_owner = owner,
            Change::TextField(field) => self.text_field = field,
            Change::Problems(problems) => self.problems = problems,
            Change::Locked(locked) => self.locked = locked,
            Change::Service(view) => {
                self.services.insert(view.name, view.document);
            }
        }
    }

    /// Apply every change of a batch, in order.
    pub fn apply_all(&mut self, changes: impl IntoIterator<Item = Change>) {
        for change in changes {
            self.apply(change);
        }
    }

    /// A window by its id.
    pub fn window(&self, id: u64) -> Option<&Window> {
        self.windows.get(&id)
    }

    /// The windows, top to bottom.
    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.stacking.iter().filter_map(|id| self.windows.get(id))
    }

    /// The window with the keyboard.
    pub fn focused(&self) -> Option<&Window> {
        self.windows().find(|window| window.focused)
    }
}

#[cfg(test)]
mod tests {
    use super::Mirror;
    use crate::{Change, Insets, Parentage, Rect, Window};

    fn window(id: u64, focused: bool) -> Window {
        Window {
            id,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
            },
            insets: Insets {
                top: 0,
                right: 0,
                bottom: 0,
                left: 0,
            },
            title: format!("window {id}"),
            focused,
            monitor: "DP-1".to_owned(),
            modal: false,
            parent: Parentage::None,
            leaving: false,
            minimized: false,
            above: false,
            icon: None,
            app_id: "foot".to_owned(),
            min: None,
            max: None,
            cramped: false,
            shown: true,
            fullscreen: false,
            maximized: false,
            style: "liquid".to_owned(),
            client_side: false,
            blur_region: false,
            x11_type: None,
            accepts_input: true,
            class: None,
            instance: None,
            content_type: "none".to_owned(),
            pid: None,
        }
    }

    #[test]
    fn windows_come_and_go_and_keep_their_order() {
        let mut mirror = Mirror::new();
        mirror.apply_all([
            Change::Window(window(1, false)),
            Change::Window(window(2, true)),
            Change::Window(window(3, false)),
        ]);
        let order: Vec<u64> = mirror.windows().map(|window| window.id).collect();
        assert_eq!(order, [3, 2, 1], "each new window on top");
        assert_eq!(mirror.focused().map(|window| window.id), Some(2));

        mirror.apply_all([Change::Stacking(vec![1, 3, 2]), Change::WindowGone(3)]);
        let order: Vec<u64> = mirror.windows().map(|window| window.id).collect();
        assert_eq!(order, [1, 2]);
        assert!(mirror.window(3).is_none());

        let mut retitled = window(1, false);
        retitled.title = "renamed".to_owned();
        mirror.apply(Change::Window(retitled));
        assert_eq!(
            mirror.window(1).map(|window| window.title.as_str()),
            Some("renamed")
        );
        assert_eq!(
            mirror.windows().count(),
            2,
            "a changed window is not a new one"
        );
    }
}
