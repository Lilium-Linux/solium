//! The session services: notifications, the tray, media and audio.

use mlua::{Lua, Table, Value};

use crate::script::lua::*;
use crate::script::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
    // Notifications (03 §3.2.11, #346). `sol.notifications{ serve = true,
    // timeouts = { low = 5000, normal = 7000, critical = 0 }, history = { max
    // = 200 } }` sets them, each key left out keeping its default and one of
    // the wrong kind named in the log; a timeout of 0 is never. With no
    // table, `sol.notifications()` answers what they are: `available`,
    // `serving`, `dnd`, `owner`, `banners` and `history`, newest first.
    // `tests::sol_notifications_sets_the_service_and_answers_its_rows`.
    sol.set(
        "notifications",
        lua.create_function(|lua, options: Option<mlua::Table>| -> mlua::Result<Value> {
            let Some(options) = options else {
                let snapshot = snapshot(lua)?;
                let view = &snapshot.notifications;
                let table = lua.create_table()?;
                table.set("available", view.available)?;
                table.set("serving", view.serving)?;
                table.set("dnd", view.dnd)?;
                for (key, rows) in [("banners", &view.banners), ("history", &view.history)] {
                    let list = lua.create_table()?;
                    for (index, row) in rows.iter().enumerate() {
                        list.set(
                            index + 1,
                            crate::services::notifications::object(row).to_lua(lua)?,
                        )?;
                    }
                    table.set(key, list)?;
                }
                if let Some(owner) = &view.owner {
                    let row = lua.create_table()?;
                    row.set("name", owner.name.clone())?;
                    row.set("pid", owner.pid)?;
                    row.set("program", owner.program.clone())?;
                    table.set("owner", row)?;
                }
                return Ok(Value::Table(table));
            };
            let config = settings::services::notifications(&lua::convert::section(Some(options))?);
            with_pending(lua, |pending| {
                pending.push(Command::Notifications(config));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // The tray (#363). `sol.tray{ serve = true }` makes Solium the session's
    // `org.kde.StatusNotifierWatcher`, so applications' tray icons come to
    // it; with no table, `sol.tray()` answers what it holds: `available`,
    // `serving`, `owner` and `items`, in the order they first answered. Its
    // actions go through `sol.act`: `tray.activate { id, x, y }` and the
    // rest.
    // `tests::sol_tray_sets_the_service_and_answers_its_items`.
    sol.set(
        "tray",
        lua.create_function(|lua, options: Option<mlua::Table>| -> mlua::Result<Value> {
            let Some(options) = options else {
                let snapshot = snapshot(lua)?;
                let view = &snapshot.tray;
                let table = lua.create_table()?;
                table.set("available", view.available)?;
                table.set("serving", view.serving)?;
                let items = lua.create_table()?;
                for (index, row) in view.items.iter().enumerate() {
                    items.set(
                        index + 1,
                        crate::services::notifications::object(row).to_lua(lua)?,
                    )?;
                }
                table.set("items", items)?;
                if let Some(owner) = &view.owner {
                    let row = lua.create_table()?;
                    row.set("name", owner.name.clone())?;
                    row.set("pid", owner.pid)?;
                    row.set("program", owner.program.clone())?;
                    table.set("owner", row)?;
                }
                return Ok(Value::Table(table));
            };
            let config = settings::services::tray(&lua::convert::section(Some(options))?);
            with_pending(lua, |pending| {
                pending.push(Command::Tray(config));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // Media (#379). `sol.media{ watch = true }` follows the session's MPRIS
    // players; with no table, `sol.media()` answers what it holds:
    // `available`, `players` in the order they appeared, and `active`, each
    // with its `position` where it is now. Its actions go through `sol.act`:
    // `media.play_pause` and the rest, on the active player with no `id`.
    // `tests::sol_media_sets_the_service_and_answers_its_players`.
    sol.set(
        "media",
        lua.create_function(|lua, options: Option<mlua::Table>| -> mlua::Result<Value> {
            let Some(options) = options else {
                let snapshot = snapshot(lua)?;
                return snapshot.media.to_json().to_lua(lua);
            };
            let config = settings::services::media(&lua::convert::section(Some(options))?);
            with_pending(lua, |pending| {
                pending.push(Command::Media(config));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // Audio (03 §3.2.5, #347). `sol.audio{ max = 1.0, step = 0.05 }` sets its
    // limits; `sol.audio()` answers what it is: `available`, `ready`,
    // `output`, `input`, `outputs` and `inputs`, every application's playback
    // stream as `streams`, and each window's streams as one, by the window's
    // id, as `windows` (#416). Either starts it. Volume and mute go through
    // `sol.act`: `audio.volume { target | window, value | by }`,
    // `audio.mute { target | window, value }`, `audio.default { endpoint }`.
    // `tests::sol_audio_sets_its_limits_and_answers_its_endpoints`.
    sol.set(
        "audio",
        lua.create_function(|lua, options: Option<mlua::Table>| -> mlua::Result<Value> {
            let Some(options) = options else {
                with_pending(lua, |pending| pending.push(Command::Audio(None)))?;
                let snapshot = snapshot(lua)?;
                let view = &snapshot.audio;
                let table = lua.create_table()?;
                table.set("available", view.available)?;
                table.set("ready", view.ready)?;
                for (key, row) in [("output", &view.output), ("input", &view.input)] {
                    if let Some(row) = row {
                        table.set(
                            key,
                            crate::services::notifications::object(row).to_lua(lua)?,
                        )?;
                    }
                }
                for (key, rows) in [
                    ("outputs", &view.outputs),
                    ("inputs", &view.inputs),
                    ("streams", &view.streams),
                ] {
                    let list = lua.create_table()?;
                    for (index, row) in rows.iter().enumerate() {
                        list.set(
                            index + 1,
                            crate::services::notifications::object(row).to_lua(lua)?,
                        )?;
                    }
                    table.set(key, list)?;
                }
                let windows = lua.create_table()?;
                for (id, row) in &view.windows {
                    windows.set(
                        *id,
                        crate::services::notifications::object(row).to_lua(lua)?,
                    )?;
                }
                table.set("windows", windows)?;
                return Ok(Value::Table(table));
            };
            let settings = settings::services::audio(&lua::convert::section(Some(options))?);
            with_pending(lua, |pending| pending.push(Command::Audio(Some(settings))))?;
            Ok(Value::Nil)
        })?,
    )?;

    // `sol.notify{ summary, body, icon, urgency, timeout, actions = { { key,
    // label, fn } } }`: one of the shell's own notifications, with no bus
    // behind it, through the same decision and model as any other (03
    // §3.2.11). An action's `fn` runs when it is invoked; `key = "default"`
    // is a click on the body.
    // `tests::sol_notify_queues_one_of_the_shells_own_with_its_actions`.
    static NOTIFY_TOKENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    sol.set(
        "notify",
        lua.create_function(|lua, options: mlua::Table| {
            let text = |key: &str| -> String {
                match options.get::<Value>(key) {
                    Ok(Value::String(text)) => text.to_string_lossy(),
                    _ => String::new(),
                }
            };
            let summary = text("summary");
            if summary.is_empty() {
                return Err(mlua::Error::runtime("sol.notify needs a summary"));
            }
            let urgency = match text("urgency").as_str() {
                "low" => crate::services::notifications::model::Urgency::Low,
                "critical" => crate::services::notifications::model::Urgency::Critical,
                _ => crate::services::notifications::model::Urgency::Normal,
            };
            let expire_timeout = match options.get::<Value>("timeout") {
                Ok(value) => number(&value)
                    .filter(|ms| *ms >= 0.0 && *ms <= f64::from(i32::MAX))
                    .map_or(-1, |ms| ms as i32),
                Err(_) => -1,
            };
            let mut actions = Vec::new();
            let functions = lua.create_table()?;
            if let Ok(Value::Table(list)) = options.get::<Value>("actions") {
                for entry in list.sequence_values::<mlua::Table>() {
                    let entry = entry?;
                    let key: String = entry.get::<Option<String>>("key")?.unwrap_or_default();
                    if key.is_empty() {
                        continue;
                    }
                    let label: String = entry.get::<Option<String>>("label")?.unwrap_or_default();
                    if let Ok(Value::Function(run)) = entry.get::<Value>("fn") {
                        functions.set(key.clone(), run)?;
                    }
                    actions.push((key, label));
                }
            }
            let token = if functions.is_empty() {
                0
            } else {
                let token = NOTIFY_TOKENS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                let sol: Table = lua.globals().get("sol")?;
                let kept: Table = sol.get("_notify_actions")?;
                kept.set(token, functions)?;
                token
            };
            let own = crate::services::notifications::Own {
                summary,
                body: text("body"),
                icon: text("icon"),
                urgency,
                actions,
                token,
                expire_timeout,
            };
            with_pending(lua, |pending| pending.push(Command::Notify(own)))
        })?,
    )?;

    Ok(())
}
