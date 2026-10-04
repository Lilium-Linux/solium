//! Workspaces, as Lua declares them and the models read them. The compositor
//! does not know what a workspace is (`lua/workspaces.lua` is the whole
//! feature), so Lua says, each time they change, and this joins what it said
//! with the windows (03 §3.2.17).
//! `state::tests::real_client::reflow_on_close::hosted::workspace_rows_count_their_windows_and_say_which_is_shown`.

use std::collections::{BTreeMap, HashSet};

use super::diff::Row;
use crate::json::Json;
use crate::state::Solium;

/// The declared shape, for a shell to draw: `Workspaces.arrangement`.
/// `models::tests::publish_models_carries_the_workspaces_the_status_and_the_arrangement`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Arrangement {
    pub(crate) kind: String,
    pub(crate) columns: u32,
    pub(crate) rows: u32,
}

/// One declared workspace. `hidden` is one a shell should not list, such as
/// a scratchpad.
/// `script::tests::sol_workspaces_declares_groups_and_windows`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Workspace {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) col: u32,
    pub(crate) row: u32,
    pub(crate) hidden: bool,
}

/// Workspaces that switch together: one group per monitor, or one for every
/// monitor; `showing` is the ids it shows now.
/// `script::tests::the_shipped_workspaces_declare_what_each_monitor_shows`,
/// `script::tests::with_workspaces_together_one_group_has_every_monitor`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Group {
    pub(crate) id: String,
    pub(crate) monitors: Vec<String>,
    pub(crate) showing: Vec<String>,
    pub(crate) workspaces: Vec<Workspace>,
}

/// What a script declared with `sol.workspaces`: the arrangement, the groups,
/// and for each window, by script id, the workspaces it is on.
/// `script::tests::sol_workspaces_declares_groups_and_windows`,
/// `state::tests::real_client::reflow_on_close::hosted::workspace_rows_count_their_windows_and_say_which_is_shown`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Declared {
    pub(crate) arrangement: Arrangement,
    pub(crate) groups: Vec<Group>,
    pub(crate) windows: BTreeMap<u64, Vec<String>>,
}

impl Declared {
    /// This declaration with every monitor and window the compositor does
    /// not have taken out, and a group left with no monitor dropped.
    /// `tests::a_declaration_naming_an_unknown_monitor_or_window_drops_them`.
    pub(crate) fn validated(mut self, monitors: &[String], windows: &[u64]) -> Self {
        for group in &mut self.groups {
            group.monitors.retain(|monitor| monitors.contains(monitor));
        }
        self.groups.retain(|group| !group.monitors.is_empty());
        self.windows.retain(|id, _| windows.contains(id));
        self
    }

    /// `Workspaces.arrangement`, as QML reads it.
    /// `models::tests::publish_models_carries_the_workspaces_the_status_and_the_arrangement`.
    pub(crate) fn arrangement_json(&self) -> Json {
        Json::Object(BTreeMap::from([
            ("kind".to_owned(), Json::Text(self.arrangement.kind.clone())),
            (
                "columns".to_owned(),
                Json::Number(f64::from(self.arrangement.columns)),
            ),
            (
                "rows".to_owned(),
                Json::Number(f64::from(self.arrangement.rows)),
            ),
        ]))
    }
}

/// Log every monitor and window `declared` names that the compositor does not
/// have, once per name for as long as `logged` lives: `workspaces.lua`
/// declares on every layout, and one mistake is one line.
/// `tests::an_unknown_monitor_or_window_is_logged_once_per_name`.
pub(crate) fn log_unknown(
    declared: &Declared,
    monitors: &[String],
    windows: &[u64],
    logged: &mut HashSet<String>,
) {
    for group in &declared.groups {
        for monitor in &group.monitors {
            if !monitors.contains(monitor) && logged.insert(format!("monitor {monitor}")) {
                tracing::warn!(
                    monitor,
                    group = group.id,
                    "sol.workspaces: no such monitor, so it was left out"
                );
            }
        }
    }
    for id in declared.windows.keys() {
        if !windows.contains(id) && logged.insert(format!("window {id}")) {
            tracing::warn!(
                window = id,
                "sol.workspaces: no such window, so it was left out"
            );
        }
    }
}

/// One row per declared workspace, keyed `<group>/<id>`, joined with the
/// windows: a window counts on a workspace it is declared on, in a group of
/// the monitor it lives on. `active` is shown by its group, and `focused` is
/// shown by the group of the active monitor (Ruling 19).
/// `state::tests::real_client::reflow_on_close::hosted::workspace_rows_count_their_windows_and_say_which_is_shown`.
#[cfg(test)]
pub(crate) fn rows(state: &Solium) -> Vec<Row> {
    joined(state, &super::windows::rows(state))
}

/// The same rows, from the window rows the frame already built, which is how
/// `publish_models` asks.
/// `models::tests::publish_models_carries_the_workspaces_the_status_and_the_arrangement`.
pub(crate) fn joined(state: &Solium, windows: &[Row]) -> Vec<Row> {
    let Some(declared) = state.workspaces.as_ref() else {
        return Vec::new();
    };
    let active = state.active_output().map(|output| output.name());
    let mut rows = Vec::new();
    for group in &declared.groups {
        let in_front = active
            .as_ref()
            .is_some_and(|name| group.monitors.contains(name));
        for workspace in &group.workspaces {
            let mine: Vec<&Row> = windows
                .iter()
                .filter(|row| {
                    row.key
                        .parse::<u64>()
                        .ok()
                        .and_then(|id| declared.windows.get(&id))
                        .is_some_and(|list| list.contains(&workspace.id))
                        && matches!(
                            row.values.get("monitor"),
                            Some(Json::Text(monitor)) if group.monitors.contains(monitor)
                        )
                })
                .collect();
            let shown = group.showing.contains(&workspace.id);
            let key = format!("{}/{}", group.id, workspace.id);
            let flag = |role: &str| {
                mine.iter()
                    .any(|row| row.values.get(role) == Some(&Json::Bool(true)))
            };
            #[expect(clippy::cast_precision_loss, reason = "a count of windows")]
            let occupied = mine.len() as f64;
            rows.push(Row {
                key: key.clone(),
                values: BTreeMap::from([
                    ("key", Json::Text(key)),
                    ("id", Json::Text(workspace.id.clone())),
                    ("name", Json::Text(workspace.name.clone())),
                    ("col", Json::Number(f64::from(workspace.col))),
                    ("row", Json::Number(f64::from(workspace.row))),
                    ("group", Json::Text(group.id.clone())),
                    (
                        "monitors",
                        Json::List(group.monitors.iter().cloned().map(Json::Text).collect()),
                    ),
                    ("active", Json::Bool(shown)),
                    ("focused", Json::Bool(shown && in_front)),
                    ("occupied", Json::Number(occupied)),
                    ("urgent", Json::Bool(flag("urgent"))),
                    ("hidden", Json::Bool(workspace.hidden)),
                    ("hasFullscreen", Json::Bool(flag("fullscreen"))),
                    (
                        "windows",
                        Json::List(
                            mine.iter()
                                .filter_map(|row| row.values.get("id").cloned())
                                .collect(),
                        ),
                    ),
                ]),
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use super::{Declared, Group, Workspace};

    fn group(id: &str, monitors: &[&str], showing: &str) -> Group {
        Group {
            id: id.to_owned(),
            monitors: monitors.iter().map(|name| (*name).to_owned()).collect(),
            showing: vec![showing.to_owned()],
            workspaces: (1..=3)
                .map(|index| Workspace {
                    id: index.to_string(),
                    name: index.to_string(),
                    col: index,
                    row: 1,
                    hidden: false,
                })
                .collect(),
        }
    }

    /// **A declaration naming a monitor or a window the compositor does not
    /// have drops them** (03 §3.2.17).
    #[test]
    fn a_declaration_naming_an_unknown_monitor_or_window_drops_them() {
        let declared = Declared {
            groups: vec![
                group("DP-1", &["DP-1", "gone"], "2"),
                group("gone", &["gone"], "1"),
            ],
            windows: BTreeMap::from([(7, vec!["2".to_owned()]), (99, vec!["1".to_owned()])]),
            ..Declared::default()
        }
        .validated(&["DP-1".to_owned()], &[7]);
        assert_eq!(
            declared.groups.len(),
            1,
            "a group left with no monitor must go"
        );
        assert_eq!(declared.groups[0].monitors, vec!["DP-1".to_owned()]);
        assert_eq!(
            declared.windows.keys().copied().collect::<Vec<_>>(),
            vec![7]
        );
    }

    /// **Each unknown monitor or window is logged once by name**, however
    /// often the same declaration comes: `workspaces.lua` declares on every
    /// layout.
    #[test]
    fn an_unknown_monitor_or_window_is_logged_once_per_name() {
        let declared = Declared {
            groups: vec![group("logged-left", &["logged-left", "logged-gone"], "1")],
            windows: BTreeMap::from([(4242, vec!["1".to_owned()])]),
            ..Declared::default()
        };
        let mut logged = HashSet::new();
        let log = crate::script::logged_while(|| {
            for _ in 0..2 {
                super::log_unknown(&declared, &["logged-left".to_owned()], &[], &mut logged);
            }
        });
        assert_eq!(
            (
                log.matches("sol.workspaces: no such monitor").count(),
                log.matches("logged-gone").count(),
                log.matches("sol.workspaces: no such window").count(),
                log.matches("4242").count(),
            ),
            (1, 1, 1, 1),
            "(monitor lines, naming it, window lines, naming it):\n{log}"
        );
    }
}
