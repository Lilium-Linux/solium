//! The protocol's commands as the compositor's own, converted in this one
//! place so that what a window manager asks for is applied as a script's
//! request always was.

use solium_wm_protocol as wire;

/// A settings document off the wire as the compositor's own, or why it is
/// not one: a node index past the end, or a table that holds itself.
pub(crate) fn doc(nodes: &[wire::DocNode]) -> Result<crate::script::Doc, String> {
    doc_at(nodes, 0, 0)
}

/// How deep a document is followed: the depth its Lua conversion allows.
const DOC_DEPTH: usize = 32;

fn doc_at(nodes: &[wire::DocNode], at: u32, depth: usize) -> Result<crate::script::Doc, String> {
    use crate::script::{Doc, DocTable};
    if depth > DOC_DEPTH {
        return Err(format!("a document nested deeper than {DOC_DEPTH}"));
    }
    let node = usize::try_from(at)
        .ok()
        .and_then(|at| nodes.get(at))
        .ok_or_else(|| format!("a document's node {at} is past its end"))?;
    Ok(match node {
        wire::DocNode::Nil => Doc::Nil,
        wire::DocNode::Boolean(on) => Doc::Bool(*on),
        wire::DocNode::Integer(whole) => Doc::Int(*whole),
        wire::DocNode::Number(number) => Doc::Num(*number),
        wire::DocNode::Text(text) => Doc::Text(text.clone()),
        wire::DocNode::Other(kind) => Doc::Other(other_kind(kind)),
        wire::DocNode::Table(table) => {
            let mut read = DocTable::default();
            for item in &table.list {
                read.list.push(doc_at(nodes, *item, depth + 1)?);
            }
            for (name, item) in &table.fields {
                read.fields
                    .insert(name.clone(), doc_at(nodes, *item, depth + 1)?);
            }
            for (key, item) in &table.numbered {
                read.numbered.insert(*key, doc_at(nodes, *item, depth + 1)?);
            }
            Doc::Table(read)
        }
    })
}

/// A kind of value a setting cannot be, by the name its language gives it,
/// as the `'static` name a [`crate::script::Doc::Other`] holds.
fn other_kind(kind: &str) -> &'static str {
    match kind {
        "function" => "function",
        "userdata" => "userdata",
        "thread" => "thread",
        "lightuserdata" => "lightuserdata",
        "string that is not UTF-8" => "string that is not UTF-8",
        _ => "value",
    }
}

/// Whether a binding at the lock screen may not ask for this: what acts on
/// a window or a workspace, or ends the session, as
/// [`crate::script::Command::reaches_past_the_lock`] says of the
/// compositor's own commands.
pub(crate) fn reaches_past_the_lock(command: &wire::Command) -> bool {
    use wire::Command as C;
    match command {
        C::Configure((section, _)) => section == "workspaces",
        C::Present(_)
        | C::PresentFrom(_)
        | C::Group(_)
        | C::PresentGroup(_)
        | C::ClearGroup(_)
        | C::Clear(_)
        | C::Touch(_)
        | C::Click(_)
        | C::Focus(_)
        | C::Warp(_)
        | C::ToggleFullscreen(_)
        | C::ToggleMaximize(_)
        | C::DragHold(_)
        | C::DragGrip(_)
        | C::ResizeHold(_)
        | C::Minimize(_)
        | C::Unminimize(_)
        | C::ShowMinimized(_)
        | C::KeepAbove(_)
        | C::Press(_)
        | C::Place(_)
        | C::Unplace(_)
        | C::Close(_)
        | C::Quit => true,
        _ => false,
    }
}

/// A command off the wire as the compositor's own commands, which may be
/// none, one or several: a settings section becomes the commands that apply
/// it. One that cannot be applied is named in the log and left out.
pub(crate) fn commands_of(command: wire::Command) -> Vec<crate::script::Command> {
    match command {
        wire::Command::Configure((section, nodes)) => match doc(&nodes) {
            Ok(options) => configure(&section, &options),
            Err(why) => {
                tracing::warn!(
                    section,
                    why,
                    "a settings document that cannot be read; left out"
                );
                Vec::new()
            }
        },
        other => {
            tracing::warn!(
                ?other,
                "a command the compositor does not apply yet; left out"
            );
            Vec::new()
        }
    }
}

/// A settings section as the commands that apply it.
fn configure(section: &str, options: &crate::script::Doc) -> Vec<crate::script::Command> {
    use crate::script::Command;
    use crate::script::settings;
    match section {
        "idle" => vec![Command::Idle(settings::idle::parse(options))],
        other => {
            tracing::warn!(section = other, "not a settings section; left out");
            Vec::new()
        }
    }
}
