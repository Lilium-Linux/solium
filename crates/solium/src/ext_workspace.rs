//! The workspaces, for a shell that is its own program (#60):
//! `ext-workspace-v1`, so a dock or a bar over `wlr-layer-shell` can show
//! which workspaces there are, which each monitor shows, and switch to one.
//!
//! **A view of what Lua declared, and nothing else.** The compositor does not
//! know what a workspace is: `workspaces.lua` declares the groups, their
//! monitors, their workspaces and which each shows with `sol.workspaces`
//! (`models::workspaces::Declared`), and this tells every bound client the
//! same, as the `Workspaces` model tells a scene. A group is a declared group
//! with its monitors as its outputs; a workspace is a declared workspace with
//! its row key as its id, its name, its column and row as its coordinates,
//! and `active` while its group shows it, `hidden` when it was declared so.
//!
//! **What a client asks is an action, and the configuration answers it.**
//! `activate` reaches Lua at the manager's `commit` as the action
//! `workspaces.go`, `{ id, monitor }`, from the surface named [`SURFACE`] --
//! exactly what a scene of the shell sends -- so `actions.lua` routes it and
//! `workspaces.lua` switches. That is the one request each workspace offers:
//! the configuration keeps a fixed set, so none is created, removed, assigned
//! or deactivated by a client, and none of those is advertised.
//!
//! Offered to every client unless `config.workspaces.offered` is false,
//! which hides the global and tells every bound manager it is finished.
//! `state::tests::real_client::reflow_on_close::ext_workspace`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay::reexports::{
    wayland_protocols::ext::workspace::v1::server::{
        ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
        ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
        ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
    },
    wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::ClientId,
    },
};

use crate::json::Json;
use crate::models::workspaces::Declared;
use crate::state::Solium;

/// The surface name a client's request reaches Lua's `surface` listeners
/// with: `sol.on("surface", function(surface, action, data) end)` hears
/// `surface == "ext-workspace"`.
pub(crate) const SURFACE: &str = "ext-workspace";

/// Whether the global is offered: read by its `can_view`.
#[derive(Debug, Clone)]
pub(crate) struct Offered(Arc<AtomicBool>);

impl Offered {
    fn on(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// One workspace as a client is told it, compared with what it was told
/// last so each change is sent once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Facts {
    name: String,
    coordinates: [u32; 2],
    state: u32,
}

/// One declared group as a client is told it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GroupFacts {
    monitors: Vec<String>,
    /// Each workspace by row key, in the order declared.
    workspaces: Vec<(String, Facts)>,
    /// The monitor `workspaces.go` is asked to switch: the group's first.
    monitor: String,
    /// Each workspace's own id, by row key, as `workspaces.go` takes it.
    ids: BTreeMap<String, String>,
}

/// What every client should be told now, by group id.
fn now(declared: Option<&Declared>) -> BTreeMap<String, GroupFacts> {
    use ext_workspace_handle_v1::State;
    let Some(declared) = declared else {
        return BTreeMap::new();
    };
    declared
        .groups
        .iter()
        .map(|group| {
            let workspaces = group
                .workspaces
                .iter()
                .map(|workspace| {
                    let mut state = State::empty();
                    if group.showing.contains(&workspace.id) {
                        state |= State::Active;
                    }
                    if workspace.hidden {
                        state |= State::Hidden;
                    }
                    (
                        format!("{}/{}", group.id, workspace.id),
                        Facts {
                            name: workspace.name.clone(),
                            coordinates: [workspace.col, workspace.row],
                            state: state.bits(),
                        },
                    )
                })
                .collect();
            let ids = group
                .workspaces
                .iter()
                .map(|workspace| {
                    (
                        format!("{}/{}", group.id, workspace.id),
                        workspace.id.clone(),
                    )
                })
                .collect();
            (
                group.id.clone(),
                GroupFacts {
                    monitors: group.monitors.clone(),
                    workspaces,
                    monitor: group.monitors.first().cloned().unwrap_or_default(),
                    ids,
                },
            )
        })
        .collect()
}

/// One bound manager: what its client was told, and the handles it holds.
#[derive(Debug)]
struct Bound {
    manager: ExtWorkspaceManagerV1,
    groups: BTreeMap<String, (ExtWorkspaceGroupHandleV1, GroupFacts)>,
    workspaces: BTreeMap<String, ExtWorkspaceHandleV1>,
    /// What it asked since its last `commit`.
    asked: Vec<(String, Json)>,
}

/// The data a workspace handle carries: its row key.
#[derive(Debug)]
pub(crate) struct WorkspaceData(String);

/// The global's state.
#[derive(Debug)]
pub(crate) struct ExtWorkspaces {
    offered: Offered,
    bound: Vec<Bound>,
}

impl ExtWorkspaces {
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let offered = Offered(Arc::new(AtomicBool::new(true)));
        display.create_global::<Solium, ExtWorkspaceManagerV1, _>(1, offered.clone());
        Self {
            offered,
            bound: Vec::new(),
        }
    }

    /// Offer the global, or take it away: hidden from a client that has
    /// not bound it, and every bound manager told it is finished.
    pub(crate) fn offer(&mut self, on: bool) {
        self.offered.0.store(on, Ordering::Relaxed);
        if !on {
            for bound in self.bound.drain(..) {
                for (handle, _) in bound.groups.values() {
                    handle.removed();
                }
                for handle in bound.workspaces.values() {
                    handle.removed();
                }
                bound.manager.finished();
            }
        }
    }

    /// Whether the global is offered.
    pub(crate) fn offered(&self) -> bool {
        self.offered.on()
    }
}

impl Solium {
    /// Tell every bound client what changed among the declared workspaces,
    /// and `done` when anything did. Every pass, beside the models, and one
    /// comparison per workspace when nothing changed.
    pub(crate) fn publish_ext_workspaces(&mut self) {
        if self.ext_workspaces.bound.is_empty() {
            return;
        }
        let now = now(self.workspaces.as_ref());
        let outputs: Vec<smithay::output::Output> = self.space.outputs().cloned().collect();
        let display = self.display_handle.clone();
        for bound in &mut self.ext_workspaces.bound {
            tell(bound, &now, &outputs, &display);
        }
    }

    /// Hand what one manager's client asked to Lua, each as an action of the
    /// surface [`SURFACE`] in a dispatch of its own, as a scene's are.
    fn settle_ext_workspace_requests(&mut self, asked: Vec<(String, Json)>) {
        for (action, data) in asked {
            let snapshot = self.snapshot();
            let Some(mut scripts) = self.scripts.take() else {
                return;
            };
            let outcome = scripts.surface_action(SURFACE, &action, &data, snapshot);
            self.scripts = Some(scripts);
            self.apply(outcome);
        }
    }
}

/// Bring one client up to `now`: groups and workspaces that went are
/// `removed`, new ones are announced and told everything, and each fact that
/// changed is sent; `done` once, if anything was.
fn tell(
    bound: &mut Bound,
    now: &BTreeMap<String, GroupFacts>,
    outputs: &[smithay::output::Output],
    display: &DisplayHandle,
) {
    use ext_workspace_handle_v1::WorkspaceCapabilities;
    let Some(client) = bound.manager.client() else {
        return;
    };
    let mut changed = false;
    let wanted: std::collections::BTreeSet<&String> = now
        .values()
        .flat_map(|group| group.workspaces.iter().map(|(key, _)| key))
        .collect();

    // Workspaces that went, first, so a group that went leaves none behind.
    let gone: Vec<String> = bound
        .workspaces
        .keys()
        .filter(|key| !wanted.contains(key))
        .cloned()
        .collect();
    for key in gone {
        if let Some(handle) = bound.workspaces.remove(&key) {
            for (group, facts) in bound.groups.values() {
                if facts.workspaces.iter().any(|(each, _)| *each == key) {
                    group.workspace_leave(&handle);
                }
            }
            handle.removed();
            changed = true;
        }
    }
    let gone: Vec<String> = bound
        .groups
        .keys()
        .filter(|id| !now.contains_key(*id))
        .cloned()
        .collect();
    for id in gone {
        if let Some((handle, _)) = bound.groups.remove(&id) {
            handle.removed();
            changed = true;
        }
    }

    for (id, group) in now {
        // The group, announced once, its monitors as they change.
        if !bound.groups.contains_key(id) {
            let Ok(handle) = client.create_resource::<ExtWorkspaceGroupHandleV1, _, Solium>(
                display,
                bound.manager.version(),
                (),
            ) else {
                continue;
            };
            bound.manager.workspace_group(&handle);
            handle.capabilities(ext_workspace_group_handle_v1::GroupCapabilities::empty());
            bound
                .groups
                .insert(id.clone(), (handle, GroupFacts::default()));
            changed = true;
        }
        let Some((handle, told)) = bound.groups.get(id).cloned() else {
            continue;
        };
        if told.monitors != group.monitors {
            for output in outputs {
                let name = output.name();
                let was = told.monitors.contains(&name);
                let is = group.monitors.contains(&name);
                if was == is {
                    continue;
                }
                for each in output.client_outputs(&client) {
                    if is {
                        handle.output_enter(&each);
                    } else {
                        handle.output_leave(&each);
                    }
                }
            }
            changed = true;
        }
        // Its workspaces.
        for (key, facts) in &group.workspaces {
            let was = told
                .workspaces
                .iter()
                .find(|(each, _)| each == key)
                .map(|(_, facts)| facts.clone());
            let workspace = if let Some(existing) = bound.workspaces.get(key) {
                existing.clone()
            } else {
                let Ok(workspace) = client.create_resource::<ExtWorkspaceHandleV1, _, Solium>(
                    display,
                    bound.manager.version(),
                    WorkspaceData(key.clone()),
                ) else {
                    continue;
                };
                bound.manager.workspace(&workspace);
                workspace.id(key.clone());
                workspace.capabilities(WorkspaceCapabilities::Activate);
                bound.workspaces.insert(key.clone(), workspace.clone());
                workspace
            };
            if was.is_none() {
                handle.workspace_enter(&workspace);
            }
            let was = was.unwrap_or_else(|| Facts {
                // Nothing sent yet, so everything differs.
                name: String::from("\u{0}"),
                coordinates: [u32::MAX; 2],
                state: u32::MAX,
            });
            if was.name != facts.name {
                workspace.name(facts.name.clone());
                changed = true;
            }
            if was.coordinates != facts.coordinates {
                workspace.coordinates(
                    facts
                        .coordinates
                        .iter()
                        .flat_map(|each| each.to_ne_bytes())
                        .collect(),
                );
                changed = true;
            }
            if was.state != facts.state {
                workspace.state(ext_workspace_handle_v1::State::from_bits_truncate(
                    facts.state,
                ));
                changed = true;
            }
        }
        if let Some((_, told)) = bound.groups.get_mut(id) {
            *told = group.clone();
        }
    }
    if changed {
        bound.manager.done();
    }
}

impl GlobalDispatch<ExtWorkspaceManagerV1, Offered> for Solium {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        offered: &Offered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        if !offered.on() {
            manager.finished();
            return;
        }
        let mut bound = Bound {
            manager,
            groups: BTreeMap::new(),
            workspaces: BTreeMap::new(),
            asked: Vec::new(),
        };
        // Everything there is now, then each change as it comes.
        let outputs: Vec<smithay::output::Output> = state.space.outputs().cloned().collect();
        tell(
            &mut bound,
            &now(state.workspaces.as_ref()),
            &outputs,
            handle,
        );
        state.ext_workspaces.bound.push(bound);
    }

    fn can_view(_client: Client, offered: &Offered) -> bool {
        offered.on()
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Commit => {
                let asked = state
                    .ext_workspaces
                    .bound
                    .iter_mut()
                    .find(|bound| bound.manager == *manager)
                    .map(|bound| std::mem::take(&mut bound.asked))
                    .unwrap_or_default();
                state.settle_ext_workspace_requests(asked);
            }
            ext_workspace_manager_v1::Request::Stop => {
                state
                    .ext_workspaces
                    .bound
                    .retain(|bound| bound.manager != *manager);
                manager.finished();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, manager: &ExtWorkspaceManagerV1, (): &()) {
        state
            .ext_workspaces
            .bound
            .retain(|bound| bound.manager != *manager);
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _group: &ExtWorkspaceGroupHandleV1,
        _request: ext_workspace_group_handle_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // `create_workspace` is not advertised: the configuration keeps a
        // fixed set. `destroy` needs nothing.
    }
}

impl Dispatch<ExtWorkspaceHandleV1, WorkspaceData> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        workspace: &ExtWorkspaceHandleV1,
        request: ext_workspace_handle_v1::Request,
        data: &WorkspaceData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // `activate` is the one advertised; the rest change nothing.
        if !matches!(request, ext_workspace_handle_v1::Request::Activate) {
            return;
        }
        let Some(bound) = state
            .ext_workspaces
            .bound
            .iter_mut()
            .find(|bound| bound.workspaces.get(&data.0) == Some(workspace))
        else {
            return;
        };
        let Some(group) = bound
            .groups
            .values()
            .map(|(_, facts)| facts)
            .find(|facts| facts.ids.contains_key(&data.0))
        else {
            return;
        };
        let id = group.ids.get(&data.0).cloned().unwrap_or_default();
        let mut asked = BTreeMap::new();
        // A number when it is one, as `workspaces.go` takes it.
        asked.insert(
            "id".to_owned(),
            id.parse::<f64>().map_or(Json::Text(id), Json::Number),
        );
        asked.insert("monitor".to_owned(), Json::Text(group.monitor.clone()));
        bound
            .asked
            .push(("workspaces.go".to_owned(), Json::Object(asked)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::workspaces::{Group, Workspace};

    #[test]
    fn a_declaration_is_told_as_groups_of_workspaces_with_their_states() {
        use ext_workspace_handle_v1::State;
        let declared = Declared {
            groups: vec![Group {
                id: "DP-1".to_owned(),
                monitors: vec!["DP-1".to_owned()],
                showing: vec!["2".to_owned()],
                workspaces: vec![
                    Workspace {
                        id: "1".to_owned(),
                        name: "one".to_owned(),
                        col: 1,
                        row: 1,
                        hidden: false,
                    },
                    Workspace {
                        id: "2".to_owned(),
                        name: "two".to_owned(),
                        col: 2,
                        row: 1,
                        hidden: false,
                    },
                    Workspace {
                        id: "scratch".to_owned(),
                        name: "scratch".to_owned(),
                        col: 3,
                        row: 1,
                        hidden: true,
                    },
                ],
            }],
            ..Declared::default()
        };
        let told = now(Some(&declared));
        let group = told.get("DP-1").expect("the group");
        assert_eq!(group.monitor, "DP-1");
        assert_eq!(
            group
                .workspaces
                .iter()
                .map(|(key, facts)| (
                    key.as_str(),
                    facts.name.as_str(),
                    facts.coordinates,
                    facts.state
                ))
                .collect::<Vec<_>>(),
            [
                ("DP-1/1", "one", [1, 1], 0),
                ("DP-1/2", "two", [2, 1], State::Active.bits()),
                ("DP-1/scratch", "scratch", [3, 1], State::Hidden.bits()),
            ]
        );
        assert_eq!(
            group.ids.get("DP-1/scratch").map(String::as_str),
            Some("scratch")
        );
        assert!(now(None).is_empty());
    }
}
