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
        wire::Command::Configure((section, nodes)) => {
            match doc(&nodes).and_then(|options| configure(&section, &options)) {
                Ok(commands) => commands,
                Err(why) => {
                    tracing::warn!(
                        section,
                        why,
                        "a settings section that cannot be read; left as it was"
                    );
                    Vec::new()
                }
            }
        }
        other => {
            tracing::warn!(
                ?other,
                "a command the compositor does not apply yet; left out"
            );
            Vec::new()
        }
    }
}

/// Why a settings section is refused, if it is: what `solium --check`
/// reports of a configuration's sections.
pub(crate) fn refusal(command: &wire::Command) -> Option<String> {
    let wire::Command::Configure((section, nodes)) = command else {
        return None;
    };
    doc(nodes)
        .and_then(|options| configure(section, &options))
        .err()
        .map(|why| format!("sol.{section} is refused: {why}"))
}

/// A settings section as the commands that apply it, each read as the
/// configuration reference documents it, or why it cannot be read, in which
/// case it applies nothing.
fn configure(
    section: &str,
    options: &crate::script::Doc,
) -> Result<Vec<crate::script::Command>, String> {
    use crate::script::settings;
    use crate::script::{Command, Doc};
    Ok(match section {
        "idle" => vec![Command::Idle(settings::idle::parse(options))],
        "auto_reload" => vec![Command::AutoReload(settings::reload::parse(options))],
        "input" => {
            let (config, shortcuts) = settings::input::parse(options)?;
            vec![Command::Input(config), Command::ShortcutsInhibit(shortcuts)]
        }
        "lock" => {
            let (settings, auth, scene) = settings::lock::parse(options);
            vec![
                Command::Lock(settings),
                Command::LockAuth(auth),
                Command::LockScene(scene),
            ]
        }
        "layer_shell" => vec![Command::LayerShell(settings::layer_shell::parse(options))],
        "frames" => vec![Command::Frames(
            settings::frames::parse(options).unwrap_or_else(|reason| {
                tracing::warn!(reason, "frames: keeping the default");
                crate::framing::Settings::default()
            }),
        )],
        "toplevels" => vec![Command::Toplevels(settings::toplevels::parse(options))],
        "x11" => vec![Command::X11(settings::x11::parse(options))],
        "focus_mode" => {
            let settings::focus::FocusMode {
                click,
                follow,
                clear_on_empty_click,
                after_close,
            } = settings::focus::parse(options);
            vec![Command::FocusMode {
                click,
                follow,
                clear_on_empty_click,
                after_close,
            }]
        }
        "pane_build" => settings::pane::build_budget(options)
            .map(Command::PaneBuild)
            .into_iter()
            .collect(),
        "loading" => vec![Command::Loading(settings::pane::loading(options)?)],
        "client_sizes" => vec![Command::ClientSizes(settings::client_sizes::parse(options))],
        "resize" => vec![Command::Resize(settings::resize::parse(options))],
        "fullscreen" => vec![Command::Fullscreen(settings::fullscreen::parse(options))],
        "cursor_theme" => vec![Command::Cursor(settings::cursor::parse(options))],
        "effects" => {
            let (rules, settings) = settings::effects::parse(options, true);
            vec![Command::Effects { rules, settings }]
        }
        "startup" => vec![Command::Startup(crate::warm::startup(options))],
        "keyboard" => {
            let request = settings::keyboard::parse(options)?;
            if request == crate::keymap::Request::default() {
                Vec::new()
            } else {
                vec![Command::Keyboard(request)]
            }
        }
        "notifications" => vec![Command::Notifications(settings::services::notifications(
            options,
        ))],
        "tray" => vec![Command::Tray(settings::services::tray(options))],
        "media" => vec![Command::Media(settings::services::media(options))],
        "audio" => vec![Command::Audio(match options {
            Doc::Nil => None,
            options => Some(settings::services::audio(options)),
        })],
        "monitors" => vec![Command::Monitors(crate::monitor::Arrangement::new(
            settings::monitors::parse(options)?,
        ))],
        "workspaces" => vec![Command::Workspaces(settings::workspaces::parse(options)?)],
        other => return Err(format!("`{other}` is not a settings section")),
    })
}
