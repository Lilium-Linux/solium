//! The models hosted scenes read, built from the compositor's own state, so
//! there is nothing to mirror. Each is diffed by key and sent to Qt as one
//! batch, every row's values written before any row is announced:
//! `tests::publish_models_carries_the_compositors_monitors_to_their_scenes`,
//! `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.

pub(crate) mod apps;
pub(crate) mod diff;
pub(crate) mod folder;
pub(crate) mod keyboard;
pub(crate) mod monitors;
pub(crate) mod pointer;
pub(crate) mod windows;
pub(crate) mod workspaces;

use crate::qml::hosted::Model;

/// What Qt last took, per model.
#[derive(Debug, Default)]
pub(crate) struct Published {
    monitors: Vec<diff::Row>,
    windows: Vec<diff::Row>,
    /// `tests::publish_models_carries_the_workspaces_the_status_and_the_arrangement`.
    workspaces: Vec<diff::Row>,
    /// `Apps`' rows (`models::apps::rows`).
    apps: Vec<diff::Row>,
    /// `Folder`'s rows (`models::folder::rows`, 04-ui.md §4.9).
    folder: Vec<diff::Row>,
    /// What `Solium.status`, `Workspaces.arrangement` and `Solium.dirs.desktop`
    /// last took.
    /// `tests::publish_models_carries_the_workspaces_the_status_and_the_arrangement`.
    status: Option<String>,
    arrangement: Option<String>,
    dirs_desktop: Option<String>,
    /// `Apps.ready`, once true never sent false again (a rescan never empties
    /// the index back to nothing worth distrusting it over).
    apps_ready: bool,
    /// `keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`.
    keyboard: keyboard::Published,
    /// `pointer::tests::a_named_shape_reaches_solium_cursor_shape`.
    pointer: pointer::Published,
}

impl crate::state::Solium {
    /// Every model's changes since what Qt last took, one batch per model:
    /// `tests::publish_models_carries_the_compositors_monitors_to_their_scenes`,
    /// `tests::a_batch_qt_cannot_take_is_sent_again_once_it_can`.
    pub(crate) fn publish_models(&mut self) {
        // The caret each decoration is told this frame:
        // `text_input::tests::only_the_pane_whose_window_has_the_caret_is_given_it`.
        self.settle_caret();
        // Apps: scanned once, lazily, on the first call this process ever
        // makes here, and again whenever a reload asked for one
        // (`state/commands.rs`'s `reload_from`). No worker thread and no
        // inotify in this version (see `apps.rs`'s module doc for why), so
        // this is the one rescan trigger there is.
        if self.apps_scan_pending {
            self.apps = crate::apps::scan(
                &crate::apps::search_dirs(),
                &crate::apps::preferred_locales(),
            );
            self.apps_scan_pending = false;
            if !self.published.apps_ready && crate::qml::hosted::set_apps_ready(true) {
                self.published.apps_ready = true;
            }
        }
        let apps = apps::rows(&self.apps);
        publish(
            Model::Apps,
            &mut self.published.apps,
            apps,
            crate::qml::hosted::apply_rows,
        );
        // `Folder`: the desktop directory, resolved once (again on a
        // reload) and rescanned wholesale whenever `folder_scan_pending`
        // asks for one or the inotify watch says something changed.
        // `with no desktop folder nothing is drawn and nothing costs
        // anything` (04-ui.md §4.9): `self.folder_dir` is `None`, the watch
        // is never armed, and every call below is one cheap option check.
        if self.folder_scan_pending {
            self.folder_dir = crate::folder::desktop_dir();
            self.folder_watcher.set_path(self.folder_dir.as_deref());
            self.folder = self
                .folder_dir
                .as_deref()
                .map(|dir| crate::folder::scan(dir, &self.folder_trust))
                .unwrap_or_default();
            self.folder_scan_pending = false;
        } else if self.folder_dir.is_some() && self.folder_watcher.poll() {
            self.folder = self
                .folder_dir
                .as_deref()
                .map(|dir| crate::folder::scan(dir, &self.folder_trust))
                .unwrap_or_default();
        }
        let folder = folder::rows(&self.folder);
        publish(
            Model::Folder,
            &mut self.published.folder,
            folder,
            crate::qml::hosted::apply_rows,
        );
        let dirs_desktop = self
            .folder_dir
            .as_deref()
            .and_then(|dir| dir.to_str())
            .unwrap_or("")
            .to_owned();
        if self.published.dirs_desktop.as_deref() != Some(dirs_desktop.as_str())
            && crate::qml::hosted::set_dirs_desktop(&dirs_desktop)
        {
            self.published.dirs_desktop = Some(dirs_desktop);
        }
        let monitors = monitors::rows(self);
        publish(
            Model::Monitors,
            &mut self.published.monitors,
            monitors,
            crate::qml::hosted::apply_rows,
        );
        let windows = windows::rows(self);
        let workspaces = workspaces::joined(self, &windows);
        publish(
            Model::Windows,
            &mut self.published.windows,
            windows,
            crate::qml::hosted::apply_rows,
        );
        // The workspaces Lua declared, `Solium.status` and
        // `Workspaces.arrangement`, each when it changed:
        // `tests::publish_models_carries_the_workspaces_the_status_and_the_arrangement`.
        publish(
            Model::Workspaces,
            &mut self.published.workspaces,
            workspaces,
            crate::qml::hosted::apply_rows,
        );
        if self.published.status.as_deref() != Some(self.status.as_str())
            && crate::qml::hosted::set_status(&self.status)
        {
            self.published.status = Some(self.status.clone());
        }
        if let Some(arrangement) = self
            .workspaces
            .as_ref()
            .map(|declared| declared.arrangement_json().render())
            && self.published.arrangement.as_deref() != Some(arrangement.as_str())
            && crate::qml::hosted::set_arrangement(&arrangement)
        {
            self.published.arrangement = Some(arrangement);
        }
        // The `Keyboard` singleton:
        // `keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`.
        let mut keyboard = std::mem::take(&mut self.published.keyboard);
        keyboard::publish(self, &mut keyboard);
        self.published.keyboard = keyboard;
        // `Solium.cursor`, while a pointer scene is configured:
        // `pointer::tests::a_named_shape_reaches_solium_cursor_shape`,
        // `pointer::tests::nothing_is_published_with_no_scene`.
        let mut pointer = std::mem::take(&mut self.published.pointer);
        pointer::publish(self, &mut pointer, crate::qml::pointer::publish);
        self.published.pointer = pointer;
    }
}

/// Send what changed from `last` to `next`. A batch Qt does not take, as
/// before it has started, leaves `last` as it was, so the next frame sends the
/// difference from what Qt really has:
/// `tests::a_batch_qt_cannot_take_is_sent_again_once_it_can`.
fn publish(
    model: Model,
    last: &mut Vec<diff::Row>,
    next: Vec<diff::Row>,
    apply: fn(Model, &str) -> bool,
) {
    let ops = diff::diff(last, &next);
    if ops.is_empty() {
        return;
    }
    if apply(model, &diff::render(&ops)) {
        *last = next;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use super::diff::{Row, diff, render};
    use super::{Model, publish};
    use crate::json::Json;

    thread_local! {
        /// What each fake `apply` was handed, and whether it took it.
        static SENT: RefCell<Vec<(Model, String, bool)>> = const { RefCell::new(Vec::new()) };
    }

    fn refused(model: Model, ops: &str) -> bool {
        SENT.with(|sent| sent.borrow_mut().push((model, ops.to_owned(), false)));
        false
    }

    fn taken(model: Model, ops: &str) -> bool {
        SENT.with(|sent| sent.borrow_mut().push((model, ops.to_owned(), true)));
        true
    }

    fn monitor(name: &str, width: f64) -> Row {
        Row {
            key: name.to_owned(),
            values: BTreeMap::from([
                ("name", Json::Text(name.to_owned())),
                ("width", Json::Number(width)),
            ]),
        }
    }

    /// **A batch Qt cannot take is sent again once it can**: refused, what Qt
    /// last took is unchanged, so the next frame sends the whole difference
    /// from it, and not only what changed since the refused one.
    #[test]
    fn a_batch_qt_cannot_take_is_sent_again_once_it_can() {
        let mut last = Vec::new();
        let first = vec![monitor("publish-1", 1920.0)];
        publish(Model::Monitors, &mut last, first.clone(), refused);
        assert!(last.is_empty(), "a refused batch was recorded as taken");

        let second = vec![monitor("publish-1", 1820.0)];
        publish(Model::Monitors, &mut last, second.clone(), taken);
        assert_eq!(last, second);
        let sent = SENT.with(|sent| sent.borrow().clone());
        assert_eq!(
            sent,
            vec![
                (Model::Monitors, render(&diff(&[], &first)), false),
                (Model::Monitors, render(&diff(&[], &second)), true),
            ],
            "the batch after a refused one must start from what Qt really has"
        );

        publish(Model::Monitors, &mut last, second, taken);
        assert_eq!(
            SENT.with(|sent| sent.borrow().len()),
            2,
            "nothing changed, so nothing is sent"
        );
    }

    /// **The compositor's monitors reach `Solium.monitor` through
    /// `publish_models`**: where the space has a monitor, a move published as
    /// a change, and an unplug as an absent row.
    #[test]
    fn publish_models_carries_the_compositors_monitors_to_their_scenes() {
        use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
        use smithay::reexports::wayland_server::Display;

        crate::qml::qt_test::on_the_qt_thread(|| {
            let (directory, mut scene) = crate::qml::hosted::tests::hosted(
                "solium-models-publish",
                r#"
                import QtQuick
                import Solium
                Item {
                    readonly property int present: Solium.monitor.present ? 1 : 0
                    readonly property int wholeX: Solium.monitor.whole.x
                    readonly property int areaWidth: Solium.monitor.area.width
                }
                "#,
                "publish-e2e-1",
            );
            let display = Display::<crate::state::Solium>::new().expect("a test display");
            let mut state = crate::state::Solium::new(display.handle());
            let output = Output::new(
                "publish-e2e-1".to_owned(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "solium".to_owned(),
                    model: "publish".to_owned(),
                },
            );
            output.change_current_state(
                Some(Mode {
                    size: (1280, 720).into(),
                    refresh: 60_000,
                }),
                None,
                Some(Scale::Fractional(1.0)),
                None,
            );
            state.space.map_output(&output, (1920, 0));

            state.publish_models();
            assert_eq!(
                (
                    scene.get_int("present"),
                    scene.get_int("wholeX"),
                    scene.get_int("areaWidth")
                ),
                (1, 1920, 1280),
                "the space's monitor did not reach the scene on it"
            );

            state.space.map_output(&output, (0, 0));
            state.publish_models();
            assert_eq!(
                scene.get_int("wholeX"),
                0,
                "a moved monitor was not published"
            );

            state.space.unmap_output(&output);
            state.publish_models();
            assert_eq!(
                scene.get_int("present"),
                0,
                "an unplugged monitor still reads present"
            );
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **`publish_models` carries Lua's workspaces, `sol.status` and the
    /// declared arrangement to every scene**: `WorkspaceList`,
    /// `Workspaces.showing(monitor)`, `Workspaces.arrangement` and
    /// `Solium.status`, from the compositor's own state.
    #[test]
    fn publish_models_carries_the_workspaces_the_status_and_the_arrangement() {
        use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
        use smithay::reexports::wayland_server::Display;

        use super::workspaces::{Arrangement, Declared, Group, Workspace};

        crate::qml::qt_test::on_the_qt_thread(|| {
            let (directory, mut scene) = crate::qml::hosted::tests::hosted(
                "solium-models-publish-workspaces",
                r#"
                import QtQuick
                import Solium
                Item {
                    WorkspaceList { id: own; monitor: "publish-ws-1" }
                    readonly property int count: own.count
                    readonly property string shown: Workspaces.showing("publish-ws-1").name
                    readonly property string kind: Workspaces.arrangement.kind || ""
                    readonly property int columns: Workspaces.arrangement.columns || 0
                    readonly property string status: Solium.status
                }
                "#,
                "publish-ws-1",
            );
            let display = Display::<crate::state::Solium>::new().expect("a test display");
            let mut state = crate::state::Solium::new(display.handle());
            let output = Output::new(
                "publish-ws-1".to_owned(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "solium".to_owned(),
                    model: "publish".to_owned(),
                },
            );
            output.change_current_state(
                Some(Mode {
                    size: (1280, 720).into(),
                    refresh: 60_000,
                }),
                None,
                Some(Scale::Fractional(1.0)),
                None,
            );
            state.space.map_output(&output, (0, 0));
            state.status = "workspace 3".to_owned();
            state.workspaces = Some(Declared {
                arrangement: Arrangement {
                    kind: "grid".to_owned(),
                    columns: 2,
                    rows: 2,
                },
                groups: vec![Group {
                    id: "publish-ws-1".to_owned(),
                    monitors: vec!["publish-ws-1".to_owned()],
                    showing: vec!["3".to_owned()],
                    workspaces: (1..=4)
                        .map(|index| Workspace {
                            id: index.to_string(),
                            name: format!("desk {index}"),
                            col: (index - 1) % 2 + 1,
                            row: (index - 1) / 2 + 1,
                            hidden: false,
                        })
                        .collect(),
                }],
                windows: std::collections::BTreeMap::new(),
            });

            state.publish_models();
            let published = (
                scene.get_int("count"),
                scene.get_string_for_test("shown"),
                scene.get_string_for_test("kind"),
                scene.get_int("columns"),
                scene.get_string_for_test("status"),
            );
            state.workspaces = None;
            state.publish_models();
            let gone = scene.get_int("count");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                published,
                (
                    4,
                    "desk 3".to_owned(),
                    "grid".to_owned(),
                    2,
                    "workspace 3".to_owned()
                ),
                "(WorkspaceList's count, showing, the arrangement's kind and columns, \
                 Solium.status)"
            );
            assert_eq!(gone, 0, "workspaces no longer declared are still listed");
        });
    }
}
