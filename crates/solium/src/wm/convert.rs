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

/// Whether a binding at the lock screen may not ask for this: act on a
/// window or a workspace, or end the session, which is how someone would
/// get to the desktop underneath a lock (see `input::escape`, which refuses
/// Ctrl+Alt+Backspace there for the same reason). A binding allowed at the
/// lock may not ask for any of it (#224); everything else it may, which is
/// what a volume, brightness or media key needs.
///
/// Every command named, with no catch-all, so that one added to the
/// protocol is decided here rather than allowed by default.
pub(crate) fn reaches_past_the_lock(command: &wire::Command) -> bool {
    use wire::Command as C;
    match command {
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
        // Declaring the workspaces moves windows between them.
        C::Configure((section, _)) => section == "workspaces",
        // `sol.act`'s verbs on windows and workspaces; `apps.launch` is a
        // spawn, and `folder` opens one. `notifications.invoke` brings the
        // sender's window forward, and a tray item's actions reach into its
        // application.
        C::Act((_, action, ..)) => {
            action.starts_with("windows.")
                || action.starts_with("workspaces.")
                || action.starts_with("tray.")
                || action == "notifications.invoke"
        }
        C::Keep(_)
        | C::Listen(_)
        | C::Declare(_)
        | C::Grab(_)
        | C::Status(_)
        | C::Decoration(_)
        | C::PaneValues(_)
        // What a frame shows, which is nothing over a lock.
        | C::FrameValues(_)
        | C::Surface(_)
        | C::SurfaceGone(_)
        | C::Power(_)
        | C::Spawn(_)
        | C::RunDetached(_)
        | C::Run(_)
        | C::Timer(_)
        | C::TimerCancel(_)
        | C::FileRead(_)
        | C::FileWrite(_)
        | C::Watch(_)
        | C::WatchCancel(_)
        | C::Store(_)
        | C::Notify(_)
        // Over a lock, it changes nothing.
        | C::LockNow
        // Nor does a peek, which is nothing over a lock.
        | C::LockPeek(_)
        // No window holds the shortcuts at the lock, and taking them back
        // gives a window nothing.
        | C::TakeShortcutsBack
        // Nor the pointer: nothing holds it at the lock.
        | C::ReleasePointer
        | C::Reload => false,
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
        command => vec![match runtime(command) {
            Ok(command) => command,
            Err(why) => {
                tracing::warn!(why, "a command the compositor cannot apply; left out");
                return Vec::new();
            }
        }],
    }
}

/// A command that acts on the desktop as the compositor's own, or why it
/// cannot be.
fn runtime(command: wire::Command) -> Result<crate::script::Command, String> {
    use crate::script::Command;
    use wire::Command as W;
    Ok(match command {
        W::Close(id) => Command::Close { id },
        W::Unplace(id) => Command::Unplace { id },
        W::ToggleFullscreen(id) => Command::ToggleFullscreen { id },
        W::ToggleMaximize(id) => Command::ToggleMaximize { id },
        W::Minimize(id) => Command::Minimize { id },
        W::Unminimize(id) => Command::Unminimize { id },
        W::ShowMinimized((id, shown)) => Command::ShowMinimized { id, shown },
        W::KeepAbove((id, on)) => Command::KeepAbove { id, on },
        W::DragHold((id, held)) => Command::DragHold { id, held },
        W::ResizeHold((id, held)) => Command::ResizeHold { id, held },
        W::DragGrip((id, along, by)) => Command::DragGrip { id, along, by },
        W::Click((x, y)) => Command::Click { x, y },
        W::Power((monitor, on)) => Command::Power { monitor, on },
        W::Decoration(name) => Command::Decoration { name },
        W::SurfaceGone(name) => Command::SurfaceGone(name),
        W::Spawn(mut argv) => {
            if argv.is_empty() {
                return Err("spawn needs a program to run".to_owned());
            }
            let program = argv.remove(0);
            Command::Spawn {
                program,
                args: argv,
            }
        }
        W::Present(present) => Command::Present {
            id: present.id,
            rect: present.rect.map(rect),
            opacity: present.opacity,
            client_opacity: present.client_opacity,
            matrix: present.matrix.map(matrix).transpose()?,
            deform: present.deform.map(deform).transpose()?,
            z: present.z,
            pivot: present.pivot,
            animation: animation(&present.motion)?,
            channels: channels(&present.channels, &present.motion)?,
        },
        W::PresentFrom(present) => Command::PresentFrom {
            id: present.id,
            rect: rect(present.rect),
            opacity: present.opacity,
            client_opacity: present.client_opacity,
            animation: animation(&present.motion)?,
            channels: channels(&present.channels, &present.motion)?,
            deform: present.deform.map(deform).transpose()?,
        },
        W::Clear((id, motion)) => Command::Clear {
            id,
            animation: animation(&motion)?,
        },
        W::Group((name, selection, motion)) => Command::Group {
            name,
            selection: selection.map(|selection| crate::script::Selection {
                windows: selection.windows,
                surfaces: selection.surfaces,
                monitors: selection.monitors,
                on: selection.on,
            }),
            animation: animation(&motion)?,
        },
        W::PresentGroup((name, shift, motion)) => Command::PresentGroup {
            name,
            to: crate::group::Shift {
                dx: shift.dx,
                dy: shift.dy,
                opacity: shift.opacity,
                matrix: matrix(shift.matrix)?,
            },
            animation: animation(&motion)?,
        },
        W::ClearGroup((name, motion)) => Command::ClearGroup {
            name,
            animation: animation(&motion)?,
        },
        W::Place(place) => Command::Place {
            id: place.id,
            rect: rect(place.rect),
            animation: animation(&place.motion)?,
            tile: place.tile,
            inside: place.inside.map(rect),
            cramped: place.cramped,
            moved: place.moved,
        },
        W::Focus((id, cause, to)) => Command::Focus {
            id,
            source: focus_source(cause),
            warp: to.map(warp).transpose()?,
        },
        W::Warp(to) => Command::Warp(warp(to)?),
        W::Touch((slot, action)) => Command::Touch {
            slot,
            action: match action {
                wire::TouchAction::Capture => crate::script::TouchAction::Capture,
                wire::TouchAction::Move(id) => crate::script::TouchAction::Move(id),
                wire::TouchAction::Resize((id, edges)) => crate::script::TouchAction::Resize(
                    id,
                    resize_edge(&edges).ok_or_else(|| format!("`{edges}` is not an edge"))?,
                ),
            },
        },
        W::Press((id, press, cause)) => Command::Press {
            id,
            press: match press {
                wire::Press::Button(button) => crate::script::Press::Button(button),
                wire::Press::Keys(names) => crate::script::Press::Keys(
                    names
                        .iter()
                        .map(|name| keysym(name))
                        .collect::<Result<_, _>>()?,
                ),
            },
            source: focus_source(cause),
        },
        W::Act((attempt, action, data, cause)) => Command::Act {
            attempt,
            action,
            data: json(&data)?,
            source: focus_source(cause),
        },
        W::PaneValues(values) => Command::PaneValues(object(&values)?),
        W::FrameValues((id, values)) => Command::FrameValues {
            id,
            fields: object(&values)?,
        },
        W::Surface(surface) => Command::Surface(Box::new(declaration(surface)?)),
        W::RunDetached(mut argv) => {
            if argv.is_empty() {
                return Err("run needs a program to run".to_owned());
            }
            let program = argv.remove(0);
            Command::Run {
                program,
                args: argv,
            }
        }
        W::Run(run) => Command::RunRead(crate::runs::Request {
            id: run.id,
            argv: run.argv,
            cwd: run.cwd,
            env: run.env,
            timeout: duration(run.timeout_ms)?,
            max_output: usize::try_from(run.max_output).unwrap_or(usize::MAX),
            lines: run.lines,
        }),
        W::Timer(timer) => Command::Timer {
            id: timer.id,
            after: duration(timer.after_ms)?,
            period: timer.period_ms.map(duration).transpose()?,
        },
        W::TimerCancel(id) => Command::TimerCancel(id),
        W::FileRead(read) => Command::FileRead(crate::files::ReadRequest {
            id: read.id,
            path: read.path.into(),
            max: usize::try_from(read.max).unwrap_or(usize::MAX),
        }),
        W::FileWrite(write) => Command::FileWrite(crate::files::WriteRequest {
            id: write.id,
            path: write.path.into(),
            data: write.data,
            report: write.report,
        }),
        W::Watch(watch) => Command::Watch {
            id: watch.id,
            path: watch.path.into(),
            shown: watch.shown,
        },
        W::WatchCancel(id) => Command::WatchCancel(id),
        W::Store((name, text)) => Command::StoreSave {
            name,
            value: json(&text)?,
        },
        W::Notify(notify) => {
            use crate::services::notifications::model::Urgency;
            Command::Notify(crate::services::notifications::Own {
                summary: notify.summary,
                body: notify.body,
                icon: notify.icon,
                urgency: match notify.urgency.as_str() {
                    "low" => Urgency::Low,
                    "normal" => Urgency::Normal,
                    "critical" => Urgency::Critical,
                    other => return Err(format!("`{other}` is not an urgency")),
                },
                actions: notify.actions,
                token: notify.token,
                expire_timeout: notify.expire_timeout,
            })
        }
        W::LockNow => Command::LockNow,
        W::LockPeek(progress) => {
            Command::LockPeek(progress.map(|progress| progress.clamp(0.0, 1.0)))
        }
        W::TakeShortcutsBack => Command::TakeShortcutsBack,
        W::ReleasePointer => Command::ReleasePointer,
        W::Reload => Command::Reload,
        W::Quit => Command::Quit,
        other => return Err(format!("{other:?} is not applied yet")),
    })
}

/// A section that is one flag.
fn flag(options: &crate::script::Doc) -> Result<bool, String> {
    match options {
        crate::script::Doc::Bool(on) => Ok(*on),
        other => Err(format!(
            "true or false, not {}",
            crate::script::doc::describe(other)
        )),
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
        "output_management" => vec![Command::OutputManagement(flag(options)?)],
        "files" => vec![Command::StoreInterval(
            settings::files::parse(options).store_interval,
        )],
        "workspaces_offered" => vec![Command::WorkspacesOffered(flag(options)?)],
        // Read once, before Qt and the session start, from the commands the
        // window manager loaded with ([`crate::script::Scripts::qml`]); a
        // later one changes nothing that is running.
        "qml" | "session" => Vec::new(),
        other => return Err(format!("`{other}` is not a settings section")),
    })
}

fn rect(rect: wire::Rect) -> crate::script::Rect {
    crate::script::Rect {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    }
}

fn curve(easing: &wire::Easing) -> crate::present::Curve {
    use crate::present::Curve;
    match easing {
        wire::Easing::Linear => Curve::Linear,
        wire::Easing::OutCubic => Curve::OutCubic,
        wire::Easing::OutBack => Curve::OutBack,
        wire::Easing::InOutQuad => Curve::InOutQuad,
        wire::Easing::InOutCubic => Curve::InOutCubic,
        wire::Easing::Bezier((x1, y1, x2, y2)) => Curve::Bezier {
            x1: *x1,
            y1: *y1,
            x2: *x2,
            y2: *y2,
        },
        wire::Easing::Spring(spring) => Curve::Spring(solium_animation::Spring {
            stiffness: spring.stiffness,
            damping: spring.damping,
            mass: spring.mass,
            initial_velocity: spring.initial_velocity,
            epsilon: spring.epsilon,
        }),
    }
}

/// A duration that crossed as milliseconds, read back to the nanosecond.
fn duration(millis: f64) -> Result<std::time::Duration, String> {
    let nanos = (millis * 1e6).round();
    if !(0.0..=u64::MAX as f64).contains(&nanos) {
        return Err(format!("{millis} ms is not a duration"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded, and checked to be within u64"
    )]
    Ok(std::time::Duration::from_nanos(nanos as u64))
}

/// JSON text as a value, or why it is not JSON.
fn json(text: &str) -> Result<crate::json::Json, String> {
    crate::json::Json::parse(text).ok_or_else(|| "data that is not JSON".to_owned())
}

/// JSON text as an object's fields.
fn object(text: &str) -> Result<std::collections::BTreeMap<String, crate::json::Json>, String> {
    match json(text)? {
        crate::json::Json::Object(fields) => Ok(fields),
        _ => Err("values that are not a JSON object".to_owned()),
    }
}

/// A scene surface's declaration as the compositor's.
fn declaration(surface: wire::Surface) -> Result<crate::scripted::Declaration, String> {
    use crate::scripted::{Declaration, Edges, KeyPolicy, Layer, On, Outside, OutsideClick};
    let outside = |outside: wire::Outside| match outside {
        wire::Outside::Swallow => Outside::Swallow,
        wire::Outside::Pass => Outside::Pass,
    };
    // Found where scenes live; one that is not there is named, since a
    // surface that silently does not appear is the hardest kind of
    // configuration mistake to find.
    let scene = crate::scripted::find_scene(&surface.scene).ok_or_else(|| {
        tracing::error!(
            surface = surface.name,
            scene = surface.scene,
            "no such QML scene"
        );
        format!("no such QML scene: {}", surface.scene)
    })?;
    Ok(Declaration {
        name: surface.name,
        scene,
        layer: match surface.layer {
            wire::Layer::Background => Layer::Background,
            wire::Layer::Bottom => Layer::Bottom,
            wire::Layer::Top => Layer::Top,
            wire::Layer::Overlay => Layer::Overlay,
        },
        on: match surface.on {
            wire::Placement::EveryMonitor => On::EveryMonitor,
            wire::Placement::Primary => On::Primary,
            wire::Placement::Monitor(name) => On::Monitor(name),
            wire::Placement::Rect((x, y, w, h)) => {
                On::Rect(smithay::utils::Rectangle::new((x, y).into(), (w, h).into()))
            }
        },
        properties: crate::scripted::Properties::new(object(&surface.properties)?),
        interactive: surface.interactive,
        reserve: Edges {
            top: surface.reserve.top,
            right: surface.reserve.right,
            bottom: surface.reserve.bottom,
            left: surface.reserve.left,
        },
        outside_click: OutsideClick {
            default: outside(surface.outside_click.default),
            named: surface
                .outside_click
                .named
                .into_iter()
                .map(|(name, policy)| (name, outside(policy)))
                .collect(),
        },
        keyboard: match surface.keyboard {
            wire::KeyPolicy::ExceptClaimed => KeyPolicy::ExceptClaimed,
            wire::KeyPolicy::All => KeyPolicy::All,
            wire::KeyPolicy::NoBindings => KeyPolicy::NoBindings,
        },
    })
}

/// A motion's timing, or why it is none: a duration that is not a number
/// of milliseconds of zero or more. `tests::a_motion_crosses_to_the_nanosecond`.
pub(crate) fn animation(motion: &wire::Motion) -> Result<crate::script::AnimationSpec, String> {
    // Read back to the nanosecond, rounded, so a duration that crossed as
    // milliseconds is the duration that was sent, not one a float's last
    // digit away from it.
    let duration = duration(motion.duration_ms)?;
    Ok(crate::script::AnimationSpec {
        duration,
        easing: curve(&motion.easing),
    })
}

fn matrix(matrix: wire::Matrix) -> Result<crate::mat4::Mat4, String> {
    let values: [f32; 16] = matrix
        .try_into()
        .map_err(|values: Vec<f32>| format!("a matrix of {} numbers, not 16", values.len()))?;
    Ok(crate::mat4::Mat4(values))
}

/// A motion of its own for each property: every one named, those left out
/// moving by the command's own motion.
fn channels(
    named: &[(String, wire::Motion)],
    motion: &wire::Motion,
) -> Result<Option<Box<crate::present::Channels<crate::script::AnimationSpec>>>, String> {
    if named.is_empty() {
        return Ok(None);
    }
    let of = |property: &str| -> Result<crate::script::AnimationSpec, String> {
        let own = named
            .iter()
            .find(|(name, _)| name == property)
            .map_or(motion, |(_, own)| own);
        animation(own)
    };
    if let Some((name, _)) = named
        .iter()
        .find(|(name, _)| !crate::script::PRESENT_MOTIONS.contains(&name.as_str()))
    {
        return Err(format!("`{name}` is not a property a present moves"));
    }
    Ok(Some(Box::new(crate::present::Channels {
        x: of("x")?,
        y: of("y")?,
        size: of("size")?,
        opacity: of("opacity")?,
        client_opacity: of("client_opacity")?,
    })))
}

pub(crate) fn deform(deform: wire::Deform) -> Result<crate::script::DeformSpec, String> {
    use crate::effect::settings::{PresentFailed, PresentReload};
    let params = doc(&deform.params)?;
    let mut read = Vec::new();
    if let crate::script::Doc::Table(table) = &params {
        for (name, value) in &table.fields {
            let value = value
                .plain()
                .as_ref()
                .and_then(crate::effect::param_value)
                .ok_or_else(|| format!("a deform's `{name}` is not a param's value"))?;
            read.push((name.clone(), value));
        }
    }
    let word = |word: Option<&str>, words: &[&str]| -> Result<(), String> {
        match word {
            Some(word) if !words.contains(&word) => {
                Err(format!("`{word}` is not one of {words:?}"))
            }
            _ => Ok(()),
        }
    };
    word(deform.failed.as_deref(), &PresentFailed::WORDS)?;
    word(deform.on_reload.as_deref(), &PresentReload::WORDS)?;
    Ok(crate::script::DeformSpec {
        effect: deform.effect,
        params: read,
        axis: deform.axis,
        progress: deform.progress,
        aim: match deform.aim {
            wire::Aim::Rect(at) => crate::script::Aim::Rect(rect(at)),
            wire::Aim::Window(id) => crate::script::Aim::Window(id),
            wire::Aim::Surface(name) => crate::script::Aim::Surface(name),
        },
        seed: deform.seed,
        failed: deform.failed.as_deref().and_then(PresentFailed::from_word),
        on_reload: deform
            .on_reload
            .as_deref()
            .and_then(PresentReload::from_word),
    })
}

fn warp(warp: wire::Warp) -> Result<crate::script::Warp, String> {
    use crate::script::{Interrupt, WarpTo};
    Ok(crate::script::Warp {
        to: match warp.to {
            wire::WarpTo::Point((x, y)) => WarpTo::Point { x, y },
            wire::WarpTo::Window((id, x, y)) => WarpTo::Window { id, at: (x, y) },
        },
        motion: warp.motion.as_ref().map(animation).transpose()?,
        interrupt: match warp.interrupt {
            wire::Interrupt::Pointer => Interrupt::Pointer,
            wire::Interrupt::Picture => Interrupt::Picture,
        },
    })
}

pub(crate) fn focus_source(cause: wire::FocusCause) -> crate::script::FocusSource {
    use crate::script::FocusSource as F;
    match cause {
        wire::FocusCause::Binding => F::Binding,
        wire::FocusCause::Direction => F::Direction,
        wire::FocusCause::Move => F::Move,
        wire::FocusCause::Click => F::Click,
        wire::FocusCause::Scroll => F::Scroll,
        wire::FocusCause::Drop => F::Drop,
        wire::FocusCause::Touch => F::Touch,
        wire::FocusCause::Hover => F::Hover,
        wire::FocusCause::Surface => F::Surface,
        wire::FocusCause::Open => F::Open,
        wire::FocusCause::Activate => F::Activate,
        wire::FocusCause::Settle => F::Settle,
        wire::FocusCause::Script => F::Script,
    }
}

/// A key by its xkb name, exactly as xkb writes it.
fn keysym(name: &str) -> Result<smithay::input::keyboard::Keysym, String> {
    use smithay::input::keyboard::{Keysym, xkb};
    match xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS) {
        Keysym::NoSymbol => Err(format!("`{name}` is not a key")),
        key => Ok(key),
    }
}

/// The edge or corner a finger resizes a window from, by its name.
pub(crate) fn resize_edge(
    name: &str,
) -> Option<smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge> {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
    Some(match name {
        "top" => ResizeEdge::Top,
        "bottom" => ResizeEdge::Bottom,
        "left" => ResizeEdge::Left,
        "right" => ResizeEdge::Right,
        "top_left" => ResizeEdge::TopLeft,
        "top_right" => ResizeEdge::TopRight,
        "bottom_left" => ResizeEdge::BottomLeft,
        "bottom_right" => ResizeEdge::BottomRight,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use solium_wm_protocol as wire;

    #[test]
    fn a_motion_crosses_to_the_nanosecond() {
        for duration in [0, 1, 240, 333, 1_000, 86_400_000] {
            let sent = crate::script::AnimationSpec {
                duration: std::time::Duration::from_millis(duration),
                easing: crate::present::Curve::OutBack,
            };
            let crossed = crate::script::lua::to_wire::motion(sent);
            assert_eq!(super::animation(&crossed), Ok(sent), "{duration} ms");
        }
        let odd = std::time::Duration::from_nanos(16_666_667);
        let crossed = crate::script::lua::to_wire::motion(crate::script::AnimationSpec {
            duration: odd,
            easing: crate::present::Curve::Linear,
        });
        assert_eq!(
            super::animation(&crossed).map(|spec| spec.duration),
            Ok(odd)
        );
        assert!(
            super::animation(&wire::Motion {
                duration_ms: -1.0,
                easing: wire::Easing::Linear
            })
            .is_err()
        );
    }
}
