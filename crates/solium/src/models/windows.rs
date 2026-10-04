//! The Windows model's rows, built from the panes. Roles carry where a window
//! lives, never where it is drawn this frame (Section 2, primitive 12).
//! `state::tests::real_client::reflow_on_close::hosted::a_window_row_carries_where_it_lives_and_its_focus`.

use std::collections::{BTreeMap, HashMap, HashSet};

use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

use super::diff::Row;
use crate::json::Json;
use crate::state::Solium;

/// One row per window a layout sees, in the order they opened.
/// `state::tests::real_client::reflow_on_close::hosted::a_window_row_carries_where_it_lives_and_its_focus`,
/// `state::tests::real_client::reflow_on_close::hosted::focus_order_is_most_recent_first`,
/// `state::tests::real_client::reflow_on_close::hosted::a_closed_window_leaves_no_gap_in_focus_order`.
pub(crate) fn rows(state: &Solium) -> Vec<Row> {
    let snapshot = state.snapshot();
    // `focusOrder` counts only the windows listed, so a window that closed
    // leaves no gap, and one never focused is last.
    // `state::tests::real_client::reflow_on_close::hosted::a_closed_window_leaves_no_gap_in_focus_order`.
    let listed: HashSet<u64> = snapshot.windows.iter().map(|info| info.id).collect();
    let ranks: HashMap<u64, usize> = state
        .focus_history
        .iter()
        .filter(|id| listed.contains(id))
        .enumerate()
        .map(|(rank, id)| (*id, rank))
        .collect();
    let mut rows: Vec<Row> = snapshot
        .windows
        .iter()
        .filter_map(|info| {
            let pane = state.panes.by_script_id(info.id)?;
            let pane_id = pane.id();
            let client = pane.client();
            let (fullscreen, maximized) = client.map_or((false, false), window_states);
            let order = ranks.get(&info.id).copied().unwrap_or(ranks.len());
            let workspace = state
                .workspaces
                .as_ref()
                .and_then(|declared| declared.windows.get(&info.id))
                .and_then(|list| list.first().cloned())
                .unwrap_or_default();
            let number = |value: u64| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "an id or a count, far below 2^53"
                )]
                let value = value as f64;
                Json::Number(value)
            };
            let shown = if client.is_none() {
                "loading"
            } else if info.leaving {
                "closing"
            } else {
                "shown"
            };
            Some(Row {
                key: info.id.to_string(),
                values: BTreeMap::from([
                    ("id", number(info.id)),
                    ("title", Json::Text(info.title.clone())),
                    ("appId", Json::Text(info.app_id.clone())),
                    (
                        "pid",
                        client
                            .and_then(|window| state.client_pid(window))
                            .map_or(Json::Number(-1.0), |pid| Json::Number(f64::from(pid))),
                    ),
                    (
                        "xwayland",
                        Json::Bool(client.is_some_and(|window| window.x11_surface().is_some())),
                    ),
                    ("monitor", Json::Text(info.monitor.clone())),
                    ("workspace", Json::Text(workspace)),
                    ("focused", Json::Bool(info.focused)),
                    (
                        "focusOrder",
                        number(u64::try_from(order).unwrap_or(u64::MAX)),
                    ),
                    ("urgent", Json::Bool(state.urgent.contains(&info.id))),
                    ("fullscreen", Json::Bool(fullscreen)),
                    ("maximized", Json::Bool(maximized)),
                    ("modal", Json::Bool(info.modal)),
                    (
                        "parent",
                        match info.parent {
                            crate::script::Parentage::Window(parent) => number(parent),
                            _ => Json::Number(0.0),
                        },
                    ),
                    ("state", Json::Text(shown.to_owned())),
                    ("onStage", Json::Bool(state.pane_on_stage(pane_id))),
                ]),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        a.key
            .len()
            .cmp(&b.key.len())
            .then_with(|| a.key.cmp(&b.key))
    });
    rows
}

/// Whether a window is fullscreen and maximised, as the compositor last set
/// it: the state its next configure carries, which is how the compositor
/// itself asks (`state::workspaces`' `fullscreen`), so nothing waits for the
/// client (Section 2, rule 3).
/// `state::tests::real_client::reflow_on_close::hosted::a_maximised_window_reads_maximized_at_once`.
fn window_states(window: &Window) -> (bool, bool) {
    if let Some(toplevel) = window.toplevel() {
        return toplevel.with_pending_state(|state| {
            (
                state.states.contains(xdg_toplevel::State::Fullscreen),
                state.states.contains(xdg_toplevel::State::Maximized),
            )
        });
    }
    window.x11_surface().map_or((false, false), |x11| {
        (x11.is_fullscreen(), x11.is_maximized())
    })
}
