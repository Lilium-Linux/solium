//! The models hosted scenes read, built from the compositor's own state, so
//! there is nothing to mirror. Each is diffed by key and sent to Qt as one
//! batch, every row's values written before any row is announced:
//! `tests::publish_models_carries_the_compositors_monitors_to_their_scenes`,
//! `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.

pub(crate) mod diff;
pub(crate) mod monitors;

use crate::qml::hosted::Model;

/// What Qt last took, per model.
#[derive(Debug, Default)]
pub(crate) struct Published {
    monitors: Vec<diff::Row>,
}

impl crate::state::Solium {
    /// Every model's changes since what Qt last took, one batch per model:
    /// `tests::publish_models_carries_the_compositors_monitors_to_their_scenes`,
    /// `tests::a_batch_qt_cannot_take_is_sent_again_once_it_can`.
    pub(crate) fn publish_models(&mut self) {
        // The caret each decoration is told this frame:
        // `text_input::tests::only_the_pane_whose_window_has_the_caret_is_given_it`.
        self.settle_caret();
        let monitors = monitors::rows(self);
        publish(
            Model::Monitors,
            &mut self.published.monitors,
            monitors,
            crate::qml::hosted::apply_rows,
        );
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
}
