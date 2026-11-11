//! What changed since the last batch: the compositor remembers what it told
//! the window manager, and each batch carries only the rows that differ,
//! whole, so the window manager's mirror is the snapshot without the
//! snapshot crossing the socket every time.

use std::collections::HashMap;
use std::rc::Rc;

use solium_wm_protocol as wire;
use wire::Change;

use super::rows as convert;
use crate::script::Snapshot;

/// What the window manager was last told.
#[derive(Debug, Default)]
pub(crate) struct Told {
    windows: HashMap<u64, wire::Window>,
    stacking: Vec<u64>,
    monitors: Vec<wire::Monitor>,
    work_area: Option<wire::Rect>,
    cursor: Option<(f64, f64)>,
    keyboard: Option<wire::Keyboard>,
    shortcuts_owner: Option<Option<u64>>,
    text_field: Option<Option<wire::TextField>>,
    problems: Option<Vec<wire::Problem>>,
    locked: Option<bool>,
    services: HashMap<&'static str, wire::Doc>,
    views: Views,
}

/// The services' views, and the installed applications, as they were when
/// each was last made into a document.
///
/// A view is shared from one snapshot to the next for as long as nothing in
/// it changes, and a snapshot is taken for every event. So the same view is
/// neither made into a document nor compared again: the installed
/// applications alone are hundreds of rows, and were most of what an event
/// cost.
#[derive(Debug, Default)]
struct Views {
    notifications: Option<Rc<crate::services::notifications::View>>,
    tray: Option<Rc<crate::services::tray::View>>,
    audio: Option<Rc<crate::services::audio::View>>,
    apps: Option<Rc<Vec<crate::apps::Entry>>>,
    media: Option<crate::services::media::Seen>,
}

/// Whether `now` is the view `then` was: the same shared value, not one
/// that reads the same.
fn same<T>(then: Option<&Rc<T>>, now: &Rc<T>) -> bool {
    then.is_some_and(|then| Rc::ptr_eq(then, now))
}

impl Told {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The changes that bring a mirror of what was told up to `snapshot`,
    /// and remember them as told.
    pub(crate) fn changes(&mut self, snapshot: &Snapshot) -> Vec<Change> {
        let mut changes = Vec::new();

        let stacking: Vec<u64> = snapshot.windows.iter().map(|window| window.id).collect();
        let gone: Vec<u64> = self
            .stacking
            .iter()
            .copied()
            .filter(|id| !stacking.contains(id))
            .collect();
        for id in gone {
            self.windows.remove(&id);
            changes.push(Change::WindowGone(id));
        }
        // What a mirror's order will be once it has these changes: a window
        // it has not seen goes on top as it arrives. The order is told only
        // when that is not the order the windows are in.
        let mut expected: Vec<u64> = self
            .stacking
            .iter()
            .copied()
            .filter(|id| stacking.contains(id))
            .collect();
        for window in &snapshot.windows {
            let row = convert::window(window);
            if self.windows.get(&window.id) != Some(&row) {
                if !self.windows.contains_key(&window.id) {
                    expected.insert(0, window.id);
                }
                self.windows.insert(window.id, row.clone());
                changes.push(Change::Window(row));
            }
        }
        if expected != stacking {
            changes.push(Change::Stacking(stacking.clone()));
        }
        self.stacking = stacking;

        let monitors: Vec<wire::Monitor> = snapshot.monitors.iter().map(convert::monitor).collect();
        if monitors != self.monitors {
            self.monitors.clone_from(&monitors);
            changes.push(Change::Monitors(monitors));
        }
        let work_area = convert::rect(snapshot.work_area);
        if self.work_area.as_ref() != Some(&work_area) {
            self.work_area = Some(work_area.clone());
            changes.push(Change::WorkArea(work_area));
        }
        if self.cursor != Some(snapshot.cursor) {
            self.cursor = Some(snapshot.cursor);
            changes.push(Change::Cursor(snapshot.cursor));
        }
        let keyboard = convert::keyboard(&snapshot.keyboard);
        if self.keyboard.as_ref() != Some(&keyboard) {
            self.keyboard = Some(keyboard.clone());
            changes.push(Change::Keyboard(keyboard));
        }
        if self.shortcuts_owner != Some(snapshot.shortcuts_owner) {
            self.shortcuts_owner = Some(snapshot.shortcuts_owner);
            changes.push(Change::ShortcutsOwner(snapshot.shortcuts_owner));
        }
        let field = snapshot.text_input.as_ref().map(convert::text_field);
        if self.text_field.as_ref() != Some(&field) {
            self.text_field = Some(field.clone());
            changes.push(Change::TextField(field));
        }
        let problems: Vec<wire::Problem> = snapshot.problems.iter().map(convert::problem).collect();
        if self.problems.as_ref() != Some(&problems) {
            self.problems = Some(problems.clone());
            changes.push(Change::Problems(problems));
        }
        if self.locked != Some(snapshot.locked) {
            self.locked = Some(snapshot.locked);
            changes.push(Change::Locked(snapshot.locked));
        }
        if !same(self.views.notifications.as_ref(), &snapshot.notifications) {
            self.views.notifications = Some(Rc::clone(&snapshot.notifications));
            let view = convert::notifications(&snapshot.notifications);
            self.service("notifications", view, &mut changes);
        }
        if !same(self.views.tray.as_ref(), &snapshot.tray) {
            self.views.tray = Some(Rc::clone(&snapshot.tray));
            let view = convert::tray(&snapshot.tray);
            self.service("tray", view, &mut changes);
        }
        // A player that is playing is somewhere else each time it is
        // looked at: the same players are the same document only when
        // there are none, or at the same moment.
        let media = &snapshot.media;
        if !self.views.media.as_ref().is_some_and(|then| {
            Rc::ptr_eq(&then.view, &media.view)
                && (media.view.players.is_empty() || then.now == media.now)
        }) {
            self.views.media = Some(media.clone());
            let view = convert::media(media);
            self.service("media", view, &mut changes);
        }
        if !same(self.views.audio.as_ref(), &snapshot.audio) {
            self.views.audio = Some(Rc::clone(&snapshot.audio));
            let view = convert::audio(&snapshot.audio);
            self.service("audio", view, &mut changes);
        }
        if !same(self.views.apps.as_ref(), &snapshot.apps) {
            self.views.apps = Some(Rc::clone(&snapshot.apps));
            let view = convert::apps(&snapshot.apps);
            self.service("apps", view, &mut changes);
        }
        changes
    }

    /// A service's view as it is now: told when it is not what was told.
    fn service(&mut self, name: &'static str, view: wire::Doc, changes: &mut Vec<Change>) {
        if self.services.get(name) != Some(&view) {
            self.services.insert(name, view.clone());
            changes.push(Change::Service((name.to_owned(), view)));
        }
    }
}

#[cfg(test)]
mod tests {
    use solium_wm_protocol::Change;
    use solium_wm_protocol::mirror::Mirror;

    use super::Told;
    use crate::script::{Rect, Snapshot, WindowInfo};
    use crate::wm::rows as convert;

    fn snapshot(windows: &[(u64, &str)]) -> Snapshot {
        Snapshot {
            windows: windows
                .iter()
                .map(|(id, title)| {
                    let mut window = WindowInfo::test(*id);
                    window.title = (*title).to_owned();
                    window.rect = Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 400.0,
                        h: 300.0,
                    };
                    window
                })
                .collect(),
            ..Snapshot::default()
        }
    }

    /// A mirror fed the changes is the snapshot, whatever came before.
    fn assert_mirrors(mirror: &Mirror, snapshot: &Snapshot) {
        let mirrored: Vec<_> = mirror.windows().cloned().collect();
        let expected: Vec<_> = snapshot.windows.iter().map(convert::window).collect();
        assert_eq!(mirrored, expected);
    }

    #[test]
    fn the_first_batch_tells_everything_and_an_unchanged_desk_nothing() {
        let mut told = Told::new();
        let desk = snapshot(&[(2, "top"), (1, "bottom")]);
        let first = told.changes(&desk);
        assert!(
            first
                .iter()
                .any(|change| matches!(change, Change::Window(window) if window.id == 2))
        );
        let mut mirror = Mirror::new();
        mirror.apply_all(first);
        assert_mirrors(&mirror, &desk);

        assert!(
            told.changes(&desk).is_empty(),
            "nothing changed, nothing sent"
        );
    }

    #[test]
    fn only_the_window_that_changed_is_sent() {
        let mut told = Told::new();
        let mut mirror = Mirror::new();
        mirror.apply_all(told.changes(&snapshot(&[(3, "a"), (2, "b"), (1, "c")])));
        let renamed = snapshot(&[(3, "a"), (2, "renamed"), (1, "c")]);
        let changes = told.changes(&renamed);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert!(matches!(&changes[0], Change::Window(window) if window.id == 2));
        mirror.apply_all(changes);
        assert_mirrors(&mirror, &renamed);
    }

    /// **A view nothing changed in is not looked at again.** The services'
    /// views and the installed applications are shared from one snapshot
    /// to the next while they stand, so the same one is neither made into
    /// a document nor told; one that was replaced is told when it reads
    /// differently, and only then.
    #[test]
    fn a_view_is_told_when_it_is_another_and_reads_differently() {
        let app = |name: &str| crate::apps::Entry {
            id: name.to_owned(),
            name: name.to_owned(),
            untranslated_name: name.to_owned(),
            generic_name: String::new(),
            icon: String::new(),
            exec: name.to_owned(),
            categories: Vec::new(),
            keywords: Vec::new(),
            terminal: false,
            path: None,
            source: std::path::PathBuf::new(),
        };
        let apps_told = |changes: &[Change]| {
            changes
                .iter()
                .filter(|change| matches!(change, Change::Service((name, _)) if name == "apps"))
                .count()
        };
        let mut desk = snapshot(&[(1, "a")]);
        desk.apps = std::rc::Rc::new(vec![app("foot")]);
        let mut told = Told::new();
        assert_eq!(apps_told(&told.changes(&desk)), 1, "told at first");
        assert_eq!(apps_told(&told.changes(&desk)), 0, "the same one");

        // Scanned again and found the same: another value that reads alike.
        desk.apps = std::rc::Rc::new(vec![app("foot")]);
        assert_eq!(apps_told(&told.changes(&desk)), 0);

        desk.apps = std::rc::Rc::new(vec![app("foot"), app("kitty")]);
        let changes = told.changes(&desk);
        assert_eq!(apps_told(&changes), 1, "one more installed");
        let mut mirror = Mirror::new();
        mirror.apply_all(changes);
        assert!(mirror.services.contains_key("apps"));
        assert_eq!(apps_told(&told.changes(&desk)), 0);
    }

    #[test]
    fn closed_raised_and_opened_windows_leave_the_mirror_in_order() {
        let mut told = Told::new();
        let mut mirror = Mirror::new();
        mirror.apply_all(told.changes(&snapshot(&[(3, "a"), (2, "b"), (1, "c")])));
        for desk in [
            snapshot(&[(1, "c"), (3, "a"), (2, "b")]),
            snapshot(&[(1, "c"), (2, "b")]),
            snapshot(&[(4, "new"), (1, "c"), (2, "b")]),
            snapshot(&[(1, "c"), (5, "under"), (4, "new"), (2, "b")]),
        ] {
            let changes = told.changes(&desk);
            mirror.apply_all(changes);
            assert_mirrors(&mirror, &desk);
        }
    }
}
