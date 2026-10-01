//! The models hosted scenes read, built from the compositor's own state, so
//! there is nothing to mirror. Each is diffed by key and sent to Qt as one
//! batch, every row's values written before any row is announced:
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
    /// Every model's changes since what Qt last took, one batch per model.
    /// `tests::a_batch_qt_cannot_take_is_sent_again_once_it_can`.
    pub(crate) fn publish_models(&mut self) {
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
}
