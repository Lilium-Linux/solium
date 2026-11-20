//! `workspaces`: the groups of workspaces a shell declares, how they are
//! laid out, and which windows are on which.

use std::collections::BTreeMap;

use super::read;
use crate::models::workspaces::{Arrangement, Declared, Group, Workspace};
use crate::script::doc::Doc;

/// Read a declaration: a workspace's `name` left out is its id, `col` and
/// `row` are 1, and it is not `hidden`. A value of the wrong kind refuses
/// the declaration, in the words Lua's conversions use.
/// `tests::sol_workspaces_declares_groups_and_windows`.
pub(crate) fn parse(declared: &Doc) -> Result<Declared, String> {
    let arrangement = match read::table(declared, "arrangement")? {
        Some(shape) => Arrangement {
            kind: read::text(shape, "kind")?.unwrap_or_default(),
            columns: read::whole::<u32>(shape, "columns", "u32")?.unwrap_or(1),
            rows: read::whole::<u32>(shape, "rows", "u32")?.unwrap_or(1),
        },
        None => Arrangement::default(),
    };
    let mut groups = Vec::new();
    if let Some(list) = read::table(declared, "groups")? {
        for group in tables(list)? {
            let mut workspaces = Vec::new();
            if let Some(list) = read::table(group, "workspaces")? {
                for workspace in tables(list)? {
                    let id = required_text(workspace, "id")?;
                    workspaces.push(Workspace {
                        name: read::text(workspace, "name")?.unwrap_or_else(|| id.clone()),
                        col: read::whole::<u32>(workspace, "col", "u32")?.unwrap_or(1),
                        row: read::whole::<u32>(workspace, "row", "u32")?.unwrap_or(1),
                        hidden: read::flag(workspace, "hidden").unwrap_or(false),
                        id,
                    });
                }
            }
            groups.push(Group {
                id: required_text(group, "id")?,
                monitors: texts(read::table(group, "monitors")?)?,
                showing: texts(read::table(group, "showing")?)?,
                workspaces,
            });
        }
    }
    let mut windows = BTreeMap::new();
    if let Some(Doc::Table(map)) = read::table(declared, "windows")? {
        if let Some((name, _)) = map.fields.iter().next() {
            return Err(read::not_a(&Doc::Text(name.clone()), "u64"));
        }
        let listed = (1_i64..).zip(&map.list);
        for (id, list) in listed.chain(map.numbered.iter().map(|(id, list)| (*id, list))) {
            let id = u64::try_from(id)
                .map_err(|_| format!("{} (out of range)", read::not_a(&Doc::Int(id), "u64")))?;
            if !matches!(list, Doc::Table(_)) {
                return Err(read::not_a(list, "table"));
            }
            windows.insert(id, texts(Some(list))?);
        }
    }
    Ok(Declared {
        arrangement,
        groups,
        windows,
    })
}

/// A list's items, each a table, or the error for the first that is not.
fn tables(list: &Doc) -> Result<Vec<&Doc>, String> {
    list.list()
        .iter()
        .map(|item| match item {
            Doc::Table(_) => Ok(item),
            other => Err(read::not_a(other, "table")),
        })
        .collect()
}

/// A list's items as text, empty for none.
fn texts(list: Option<&Doc>) -> Result<Vec<String>, String> {
    list.map_or_else(
        || Ok(Vec::new()),
        |list| {
            list.list()
                .iter()
                .map(|item| read::text_of(item)?.ok_or_else(|| read::not_a(item, "String")))
                .collect()
        },
    )
}

/// Text under `key` that must be there.
fn required_text(table: &Doc, key: &str) -> Result<String, String> {
    read::text(table, key)?.ok_or_else(|| read::not_a(table.get(key), "String"))
}
