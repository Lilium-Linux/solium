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

use std::rc::Rc;

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
    /// `Notifications.banners` and `.history`, and its own values, as last
    /// taken (#346).
    notification_banners: Vec<diff::Row>,
    notification_history: Vec<diff::Row>,
    notifications_state: Option<String>,
    /// `Audio.outputs` and `.inputs`, and its own values, as last taken (#347).
    audio_outputs: Vec<diff::Row>,
    audio_inputs: Vec<diff::Row>,
    /// `Audio.streams` (#416).
    audio_streams: Vec<diff::Row>,
    audio_state: Option<String>,
    /// `Tray.items`, and its own values, as last taken (#363).
    tray: Vec<diff::Row>,
    tray_state: Option<String>,
    /// `Media.players`, and its own values, as last taken (#379).
    media: Vec<diff::Row>,
    media_state: Option<String>,
    /// What `Solium.status`, `Workspaces.arrangement` and `Solium.dirs.desktop`
    /// last took.
    /// `tests::publish_models_carries_the_workspaces_the_status_and_the_arrangement`.
    status: Option<String>,
    arrangement: Option<String>,
    dirs_desktop: Option<String>,
    /// `Apps.ready`, once true never sent false again (a rescan never empties
    /// the index back to nothing worth distrusting it over).
    apps_ready: bool,
    /// What each shared list or service view was when Qt last took the rows
    /// made from it: a pass whose list or view is the same `Rc` builds no
    /// rows from it, and diffs and sends nothing (#392).
    /// `tests::an_unchanged_source_is_not_made_into_rows_again`.
    sources: Sources,
    /// `keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`.
    keyboard: keyboard::Published,
    /// `pointer::tests::a_named_shape_reaches_solium_cursor_shape`.
    pointer: pointer::Published,
}

/// The shared lists and service views `publish_models` last made rows from,
/// and Qt took.
#[derive(Debug, Default)]
struct Sources {
    apps: Option<Rc<Vec<crate::apps::Entry>>>,
    folder: Option<Rc<Vec<crate::folder::Entry>>>,
    notifications: Option<Rc<crate::services::notifications::View>>,
    audio: Option<Rc<crate::services::audio::View>>,
    tray: Option<Rc<crate::services::tray::View>>,
    media: Option<Rc<crate::services::media::View>>,
}

/// Send what is made from `now` with `send`, unless Qt has already taken
/// what was made from this very `now`: the same `Rc`, which `taken` holds,
/// so its address cannot be freed and reused by another. `taken` becomes
/// `now` once `send` says Qt has all of it; refused, the next pass sends
/// again. `tests::an_unchanged_source_is_not_made_into_rows_again`.
fn publish_from<T>(taken: &mut Option<Rc<T>>, now: &Rc<T>, send: impl FnOnce(&T) -> bool) {
    if taken.as_ref().is_some_and(|taken| Rc::ptr_eq(taken, now)) {
        return;
    }
    if send(now) {
        *taken = Some(Rc::clone(now));
    }
}

impl crate::state::Solium {
    /// Every model's changes since what Qt last took, one batch per model:
    /// `tests::publish_models_carries_the_compositors_monitors_to_their_scenes`,
    /// `tests::a_batch_qt_cannot_take_is_sent_again_once_it_can`.
    pub(crate) fn publish_models(&mut self) {
        // The caret each decoration is told this frame:
        // `text_input::tests::only_the_pane_whose_window_has_the_caret_is_given_it`.
        self.settle_caret();
        // What Solium's own lock shows of its password: never the password.
        // `lock_auth::tests::typed_text_never_reaches_the_lock_scene`.
        crate::lock_auth::publish(self);
        // Every scene reads the session's own models, locked or not: a scene
        // of the session is not drawn behind Solium's own lock, and emptying
        // its models there would destroy every item it builds from them, to
        // be built again at the unlock, over the lock's exit (#281's slide).
        // What is drawn over the lock reads empty ones of its own at all
        // times: the lock's scenes and the pointer's (`qml/attached.h`,
        // `solium_redacted`), the monitors and the keyboard as anywhere.
        // `lock_auth::tests::a_lock_scene_reads_only_what_a_lock_may_show`,
        // `lock_auth::tests::the_pointer_scene_reads_only_what_a_lock_may_show`,
        // `state::tests::real_client::lock_focus::native::a_session_scene_keeps_its_rows_through_a_native_lock`.

        // Apps: scanned once, lazily, on the first call this process ever
        // makes here, and again whenever a reload asked for one
        // (`state/commands.rs`'s `reload_from`). No worker thread and no
        // inotify in this version (see `apps.rs`'s module doc for why), so
        // this is the one rescan trigger there is.
        if self.apps_scan_pending {
            self.apps = Rc::new(crate::apps::scan(
                &crate::apps::search_dirs(),
                &crate::apps::preferred_locales(),
            ));
            self.apps_scan_pending = false;
            if !self.published.apps_ready && crate::qml::hosted::set_apps_ready(true) {
                self.published.apps_ready = true;
            }
        }
        // Rows only from a list Qt has not taken: a scan is a new list.
        publish_from(&mut self.published.sources.apps, &self.apps, |list| {
            publish(
                Model::Apps,
                &mut self.published.apps,
                apps::rows(list),
                crate::qml::hosted::apply_rows,
            )
        });
        // Notifications (#346): the service's rows and its own values, read
        // by session scenes and by the lock's alike (notifications show on
        // the lock: Illia's 2026-10-01 answer).
        // `models::tests::publish_models_carries_notifications_to_their_rows`.
        let rows = |rows: &[std::collections::BTreeMap<&'static str, crate::json::Json>]| {
            rows.iter()
                .map(|values| diff::Row {
                    key: values
                        .get("id")
                        .and_then(crate::json::Json::as_u64)
                        .unwrap_or_default()
                        .to_string(),
                    values: values.clone(),
                })
                .collect::<Vec<_>>()
        };
        let view = self.services.notifications.view();
        publish_from(&mut self.published.sources.notifications, &view, |view| {
            let banners = publish(
                Model::NotificationBanners,
                &mut self.published.notification_banners,
                rows(&view.banners),
                crate::qml::hosted::apply_rows,
            );
            let history = publish(
                Model::NotificationHistory,
                &mut self.published.notification_history,
                rows(&view.history),
                crate::qml::hosted::apply_rows,
            );
            let state = notifications_state(view);
            let values = self.published.notifications_state.as_deref() == Some(state.as_str())
                || crate::qml::hosted::set_notifications_state(&state);
            if values {
                self.published.notifications_state = Some(state);
            }
            banners && history && values
        });
        // Audio (#347): its endpoints, and the default output and input.
        let audio = self.services.audio.view();
        publish_from(&mut self.published.sources.audio, &audio, |audio| {
            let outputs = publish(
                Model::AudioOutputs,
                &mut self.published.audio_outputs,
                rows(&audio.outputs),
                crate::qml::hosted::apply_rows,
            );
            let inputs = publish(
                Model::AudioInputs,
                &mut self.published.audio_inputs,
                rows(&audio.inputs),
                crate::qml::hosted::apply_rows,
            );
            let streams = publish(
                Model::AudioStreams,
                &mut self.published.audio_streams,
                rows(&audio.streams),
                crate::qml::hosted::apply_rows,
            );
            let state = audio_state(audio);
            let values = self.published.audio_state.as_deref() == Some(state.as_str())
                || crate::qml::hosted::set_audio_state(&state);
            if values {
                self.published.audio_state = Some(state);
            }
            outputs && inputs && streams && values
        });
        // The tray (#363): its items, and its own values.
        let tray = self.services.tray.view();
        publish_from(&mut self.published.sources.tray, &tray, |tray| {
            let items = publish(
                Model::Tray,
                &mut self.published.tray,
                rows(&tray.items),
                crate::qml::hosted::apply_rows,
            );
            let state = tray_state(tray);
            let values = self.published.tray_state.as_deref() == Some(state.as_str())
                || crate::qml::hosted::set_tray_state(&state);
            if values {
                self.published.tray_state = Some(state);
            }
            items && values
        });
        // Media (#379): its players, and its own values.
        let media = self.services.media.view();
        publish_from(&mut self.published.sources.media, &media, |media| {
            let players = publish(
                Model::Media,
                &mut self.published.media,
                rows(&media.players),
                crate::qml::hosted::apply_rows,
            );
            let state = media_state(media);
            let values = self.published.media_state.as_deref() == Some(state.as_str())
                || crate::qml::hosted::set_media_state(&state);
            if values {
                self.published.media_state = Some(state);
            }
            players && values
        });
        // `Folder`: the desktop directory, resolved once (again on a
        // reload) and rescanned wholesale whenever `folder_scan_pending`
        // asks for one or the inotify watch says something changed.
        // `with no desktop folder nothing is drawn and nothing costs
        // anything` (04-ui.md §4.9): `self.folder_dir` is `None`, the watch
        // is never armed, and every call below is one cheap option check.
        if self.folder_scan_pending {
            self.folder_dir = crate::folder::desktop_dir();
            self.folder_watcher.set_path(self.folder_dir.as_deref());
            self.folder = Rc::new(
                self.folder_dir
                    .as_deref()
                    .map(|dir| crate::folder::scan(dir, &self.folder_trust))
                    .unwrap_or_default(),
            );
            self.folder_scan_pending = false;
        } else if self.folder_dir.is_some()
            && (std::mem::take(&mut self.folder_changed) | self.folder_watcher.poll())
        {
            self.folder = Rc::new(
                self.folder_dir
                    .as_deref()
                    .map(|dir| crate::folder::scan(dir, &self.folder_trust))
                    .unwrap_or_default(),
            );
        }
        publish_from(&mut self.published.sources.folder, &self.folder, |list| {
            publish(
                Model::Folder,
                &mut self.published.folder,
                folder::rows(list),
                crate::qml::hosted::apply_rows,
            )
        });
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
        let status = self.status.as_str();
        if self.published.status.as_deref() != Some(status)
            && crate::qml::hosted::set_status(status)
        {
            self.published.status = Some(status.to_owned());
        }
        let arrangement = self
            .workspaces
            .as_ref()
            .map(|declared| declared.arrangement_json().render());
        if let Some(arrangement) = arrangement
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

    /// Take back from Qt the session's rows, status and arrangement this
    /// `Solium` published. Qt's stores are the process's, shared by every
    /// test on the Qt thread, so a test that publishes a session leaves none
    /// of it for the next.
    #[cfg(test)]
    pub(crate) fn unpublish_session_for_test(&mut self) {
        for (model, last) in [
            (Model::Apps, &mut self.published.apps),
            (Model::Folder, &mut self.published.folder),
            (Model::Windows, &mut self.published.windows),
            (Model::Workspaces, &mut self.published.workspaces),
            (Model::Tray, &mut self.published.tray),
            (Model::Media, &mut self.published.media),
        ] {
            publish(model, last, Vec::new(), crate::qml::hosted::apply_rows);
        }
        // What the rows were made from is gone from Qt with them.
        self.published.sources = Sources::default();
        if crate::qml::hosted::set_status("") {
            self.published.status = Some(String::new());
        }
        if crate::qml::hosted::set_arrangement("{}") {
            self.published.arrangement = Some("{}".to_owned());
        }
        if crate::qml::hosted::set_dirs_desktop("") {
            self.published.dirs_desktop = Some(String::new());
        }
    }
}

/// Send what changed from `last` to `next`. A batch Qt does not take, as
/// before it has started, leaves `last` as it was, so the next frame sends the
/// difference from what Qt really has:
/// `tests::a_batch_qt_cannot_take_is_sent_again_once_it_can`.
/// `Audio`'s own values, as the JSON object the host reads.
fn audio_state(view: &crate::services::audio::View) -> String {
    use crate::json::Json;
    let facade = |row: &Option<std::collections::BTreeMap<&'static str, Json>>| {
        row.as_ref()
            .map_or(Json::Null, crate::services::notifications::object)
    };
    // Each window's streams as one, by the window's id, for the facades
    // `Audio.forWindow` hands out (#416).
    let windows = view
        .windows
        .iter()
        .map(|(id, row)| (id.to_string(), crate::services::notifications::object(row)))
        .collect();
    Json::Object(std::collections::BTreeMap::from([
        ("available".to_owned(), Json::Bool(view.available)),
        ("ready".to_owned(), Json::Bool(view.ready)),
        ("output".to_owned(), facade(&view.output)),
        ("input".to_owned(), facade(&view.input)),
        ("windows".to_owned(), Json::Object(windows)),
    ]))
    .render()
}

/// `Tray`'s own values, as the JSON object the host reads.
fn tray_state(view: &crate::services::tray::View) -> String {
    use crate::json::Json;
    Json::Object(std::collections::BTreeMap::from([
        ("available".to_owned(), Json::Bool(view.available)),
        ("serving".to_owned(), Json::Bool(view.serving)),
        ("owner".to_owned(), owner_json(view.owner.as_ref())),
    ]))
    .render()
}

/// `Media`'s own values, as the JSON object the host reads.
fn media_state(view: &crate::services::media::View) -> String {
    use crate::json::Json;
    Json::Object(std::collections::BTreeMap::from([
        ("available".to_owned(), Json::Bool(view.available)),
        (
            "positionInterval".to_owned(),
            Json::Number(f64::from(view.position_interval_ms)),
        ),
    ]))
    .render()
}

/// Who has a service's name when Solium does not, or `null`.
fn owner_json(owner: Option<&crate::services::notifications::server::Owner>) -> crate::json::Json {
    use crate::json::Json;
    owner.map_or(Json::Null, |owner| {
        Json::Object(std::collections::BTreeMap::from([
            ("name".to_owned(), Json::Text(owner.name.clone())),
            (
                "pid".to_owned(),
                owner
                    .pid
                    .map_or(Json::Null, |pid| Json::Number(f64::from(pid))),
            ),
            ("program".to_owned(), Json::Text(owner.program.clone())),
        ]))
    })
}

/// `Notifications`' own values, as the JSON object the host reads.
fn notifications_state(view: &crate::services::notifications::View) -> String {
    use crate::json::Json;
    Json::Object(std::collections::BTreeMap::from([
        ("available".to_owned(), Json::Bool(view.available)),
        ("serving".to_owned(), Json::Bool(view.serving)),
        ("dnd".to_owned(), Json::Bool(view.dnd)),
        ("owner".to_owned(), owner_json(view.owner.as_ref())),
    ]))
    .render()
}

/// Whether Qt has `next` now: nothing to send, or the batch was taken.
fn publish(
    model: Model,
    last: &mut Vec<diff::Row>,
    next: Vec<diff::Row>,
    apply: fn(Model, &str) -> bool,
) -> bool {
    let ops = diff::diff(last, &next);
    if ops.is_empty() {
        return true;
    }
    let taken = apply(model, &diff::render(&ops));
    if taken {
        *last = next;
    }
    taken
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use super::diff::{Row, diff, render};
    use super::{Model, publish, publish_from};
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

    /// **An unchanged source is not made into rows again** (#392): the app
    /// list, the desktop folder and each service's view are shared and
    /// replaced whole when they change, so a pass that finds the one Qt took
    /// builds nothing from it; a new one is sent, and one Qt refused is sent
    /// again on the next pass.
    #[test]
    fn an_unchanged_source_is_not_made_into_rows_again() {
        let mut taken = None;
        let mut made = Vec::new();
        let list = std::rc::Rc::new(vec!["firefox"]);
        publish_from(&mut taken, &list, |list| {
            made.push(list.clone());
            true
        });
        publish_from(&mut taken, &list, |list| {
            made.push(list.clone());
            true
        });
        assert_eq!(
            made,
            vec![vec!["firefox"]],
            "the same list was made into rows twice"
        );

        let rescanned = std::rc::Rc::new(vec!["firefox", "foot"]);
        publish_from(&mut taken, &rescanned, |_| false);
        publish_from(&mut taken, &rescanned, |list| {
            made.push(list.clone());
            true
        });
        assert_eq!(
            made,
            vec![vec!["firefox"], vec!["firefox", "foot"]],
            "a new list, refused once, was not sent again"
        );
        publish_from(&mut taken, &rescanned, |list| {
            made.push(list.clone());
            true
        });
        assert_eq!(made.len(), 2, "a list Qt took was sent again");
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
