//! The settings sections, each read from a document in `script/settings/`.

use mlua::{Lua, Table, Value};

use crate::*;

/// Hand a settings section over as a document, for the compositor to read
/// as its configuration reference documents it.
pub(crate) fn configure(
    lua: &Lua,
    section: &str,
    options: solium_wm_protocol::Doc,
) -> mlua::Result<()> {
    with_pending(lua, |pending| {
        pending.push(solium_wm_protocol::Command::Configure(
            solium_wm_protocol::Configure {
                section: section.to_owned(),
                settings: options,
            },
        ));
    })
}

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
    // Effects on parts of windows: `sol.effects{ rules = { … } }`, parsed
    // here and applied whole, or not at all
    // (`state::tests::a_broken_rule_keeps_the_rules_that_ran`), with the
    // engine's own keys beside them (Ruling 28,
    // `tests::sol_effects_reaches_the_compositor_with_its_settings`).
    sol.set(
        "effects",
        lua.create_function(|lua, options: Option<Table>| {
            configure(lua, "effects", convert::wire_section(options)?)
        })?,
    )?;

    // How the compositor starts: the first desktop frame's gate
    // (`warm::Warm`), `startup.first_frame`, read and checked here and
    // applied at once (`tests::sol_startup_reaches_the_compositor`).
    sol.set(
        "startup",
        lua.create_function(|lua, options: Option<Table>| {
            configure(lua, "startup", convert::wire_section(options)?)
        })?,
    )?;

    // The idle lock, the idle blank and what a window on a dark screen is
    // told: `config.idle`, handed over by `init.lua`. `lock_after` and
    // `screens_off_after` are in seconds and `off_frame_interval` in
    // milliseconds; 0 is "never" for each. An absent table or key keeps the default, for the reason
    // `sol.resize` gives, and a value that is not a number of zero or more is
    // named in the log and the default kept.
    // `the_idle_blank_turns_every_screen_off_after_the_timeout`.
    // `dbus_inhibit` is true or false, and anything else is named and the
    // default kept: `idle_dbus_inhibit_false_owns_nothing`.
    sol.set(
        "idle",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "idle", convert::wire_section(options)?)
        })?,
    )?;
    // Automatic reload (#223): `config.reload`, handed over by `init.lua`.
    // `automatic` is true or false, default true; `quiet_ms` is milliseconds
    // of zero or more, default 300 -- long enough that an editor's own
    // "write a temp file, rename it over the original" lands in one quiet
    // period, short enough that a single save still feels instant. Either
    // left out, or of the wrong kind, keeps the default, named in the log --
    // the same contract every other `sol.<setting>` here keeps. Applied at
    // once: `state/commands.rs`'s `configure_autoreload` arms or disarms the
    // watch to match, so toggling this from a binding of your own works
    // mid-session and not only from `config.lua`.
    sol.set(
        "auto_reload",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "auto_reload", convert::wire_section(options)?)
        })?,
    )?;

    // libinput device settings (#157): a default per device type, and
    // overrides matched by name or by vendor/product. `config.lua`'s `input`
    // section, handed over as `sol.input(config.input)`.
    //
    // Each device-type key -- `touchpad`, `mouse`, `keyboard`, `touchscreen`,
    // `tablet_tool`, `tablet_pad`, `switch` -- is a table of the options
    // listed on `solium::input::devices::Settings`; left out, or not a table,
    // every device of that type keeps libinput's own default. `devices` is a
    // list of overrides, each a match -- `name` (a case-insensitive
    // substring), `vendor` and `product` (as `libinput list-devices` or
    // `lsusb` print them), at least one of the three -- plus any of the same
    // options. Overrides apply in list order, over the type default and over
    // each other, field by field: a later one changes only the fields it
    // names, and a field none of them names keeps the type default. Applied
    // to every device already connected and to each one `DeviceAdded` hands
    // over later, and reapplied, from whatever the configuration now says,
    // on every reload -- the live side of this is `tty.rs` and
    // `solium::input::devices::Registry`, since only a backend has real
    // devices to apply it to.
    // `the_shipped_configuration_configures_a_touchpad_and_a_mouse`,
    // `a_devices_entry_in_user_lua_overrides_one_field_of_the_matched_device`,
    // `sol_input_warns_on_an_unrecognised_option_value_and_keeps_it_unset`.
    sol.set(
        "input",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "input", convert::wire_section(options)?)
        })?,
    )?;

    // logind's `Lock` and sleep signals (#153): `config.lock`, handed over by
    // `init.lua`. `command` is a program and its arguments, split on
    // whitespace with no quoting, like an autostart `Exec=` line; absent or
    // not a string, nothing runs on `Lock` or before sleep, which is the
    // default — see `logind.rs` for why. `before_sleep` and `logind_unlock`
    // are true or false, default true; anything else is named in the log and
    // the default kept.
    // The rest is how Solium's own lock checks a password (#281),
    // `lock_auth_settings`, and what it shows, `lock_scene_settings`.
    sol.set(
        "lock",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "lock", convert::wire_section(options)?)
        })?,
    )?;

    // Which of a client's layer surfaces below the windows take the pointer
    // and touch (#461): `config.layer_shell`, handed over by `init.lua`.
    // `input_below` is `true` (every one), `false` (none), or a list of
    // namespaces, each exact, `*`, or a prefix ending in `*`. Anything else
    // keeps the default, every one, and says so, the same as `sol.x11` keeps
    // its list -- see `layer::Settings::default`.
    // `tests::sol_layer_shell_reads_input_below_as_a_flag_or_a_list_of_namespaces`.
    sol.set(
        "layer_shell",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "layer_shell", convert::wire_section(options)?)
        })?,
    )?;

    // Who draws a Wayland window's frame (#462): `config.frames`, handed over
    // by `init.lua`. `policy` is "prefer", "force" or "allow"; `rules` is a
    // list of `{ match = { app_id = ... }, policy = ... }`, each `app_id`
    // exact, `*`, or a prefix ending in `*`, the last matching one winning.
    // Anything else is refused whole and the default kept, with the reason
    // logged: half a policy is a frame on the windows nobody meant.
    // `tests::sol_frames_reads_the_policy_and_its_rules`.
    sol.set(
        "frames",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "frames", convert::wire_section(options)?)
        })?,
    )?;

    // Which window protocols a shell that is its own program is offered
    // (#50): `config.toplevels`, handed over by `init.lua`. `list` and
    // `manage` are each true or false; one left out, or not a boolean, keeps
    // its default, true, and a wrong type says so. `capture` (#47) is
    // `"client"`, `"pane"` or `false`, and anything else keeps `"client"`.
    // `tests::sol_toplevels_reads_each_protocol_as_a_flag`.
    sol.set(
        "toplevels",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "toplevels", convert::wire_section(options)?)
        })?,
    )?;

    // Which `WM_CLASS` names are refused a tile, decoration or bar entry
    // outright (#221): `config.x11`, handed over by `init.lua`. `hidden` is a
    // list of strings, matched case-insensitively against either `WM_CLASS`
    // field (`X11Surface::class()` or `X11Surface::instance()`); absent, or
    // not a table of strings, keeps the default list rather than emptying
    // it, the same as `sol.resize` and `sol.idle` keep theirs — see
    // `xwayland::Settings::default`.
    sol.set(
        "x11",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "x11", convert::wire_section(options)?)
        })?,
    )?;

    // The input profile's focus policy, overridden from Lua (#219):
    // `config.focus`, which `lua/modes.lua` hands over on every mode change
    // as well as at load, same as `sol.idle` above. Unlike `sol.idle`, a key
    // left out -- or the whole table left out -- is not a fixed default but
    // the machine's own answer, from its form factor (`SOLIUM_FORM_FACTOR`,
    // #159), which the compositor works out when it applies the command. And
    // never the answer the call before left, which is what a mode naming
    // nothing used to inherit (#128): see `Command::FocusMode`.
    // `sol_focus_mode_leaves_a_key_out_to_the_machine`.
    sol.set(
        "focus_mode",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "focus_mode", convert::wire_section(options)?)
        })?,
    )?;

    sol.set(
        "pane",
        lua.create_function(|lua, name: Option<String>| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Decoration(name));
            })
        })?,
    )?;

    // How long a pass may spend building the frames of a new pane style, in
    // milliseconds (#226): `config.pane_build`, handed over by `init.lua`.
    // Nil says nothing; anything but a number of 0 or more is named in the
    // log and changes nothing. `tests::sol_pane_build_hands_the_budget_over`.
    sol.set(
        "pane_build",
        lua.create_function(|lua, millis: Value| {
            configure(lua, "pane_build", convert::wire_doc_of(&millis)?)
        })?,
    )?;

    // Values for every pane's layers, read there as one object, `values`: the
    // configuration's way to hand its own settings to the QML that draws by
    // them, whatever they are. Merged by key.
    // `decoration::tests::pane_values_merge_by_key_and_count_only_changes`.
    sol.set(
        "pane_values",
        lua.create_function(|lua, values: mlua::Table| {
            let fields = solium_json::Json::object_from_lua(&values)?;
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::PaneValues(to_wire::object(
                    &fields,
                )));
            })
        })?,
    )?;

    // Values for one window's frame alone, read there as `windowValues`
    // (#417): what the configuration keeps for that window and the frame
    // shows, its opacity for one. Merged by key, as `sol.pane_values` is,
    // and gone with the window.
    // `tests::sol_frame_hands_one_window_its_values`.
    sol.set(
        "frame",
        lua.create_function(|lua, (id, values): (u64, mlua::Table)| {
            let fields = solium_json::Json::object_from_lua(&values)?;
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::FrameValues(
                    solium_wm_protocol::FrameValues {
                        id,
                        values: to_wire::object(&fields),
                    },
                ));
            })
        })?,
    )?;

    // The old name for `sol.pane`. A style used to be a single QML file and
    // is now a folder; the name changed with it. Kept so no configuration
    // written before the change breaks, and cheap enough to keep until there
    // is a reason to remove it.
    sol.set("decoration", sol.get::<mlua::Function>("pane")?)?;

    // Everything `sol.pane` could be handed, asked of the compositor, which
    // looks in the directories it actually resolves against rather than at a
    // list kept in Lua. Same principle as `parse_easing`: the names come from
    // the machinery, so a style someone writes is offerable the moment it
    // exists and `lua/tweaks.lua` has nothing to edit.
    //
    // `{ name = "...", kind = "bundle" | "file" }` per entry, sorted, bundles
    // first, and each name appearing once -- the shadowing is already applied,
    // so a script can offer the list as it stands without implying a choice
    // the compositor would not make.
    sol.set(
        "decorations",
        lua.create_function(|lua, ()| {
            let list = lua.create_table()?;
            let styles = match ask(lua, solium_wm_protocol::Query::Styles) {
                Some(solium_wm_protocol::Reply::Styles(styles)) => styles,
                _ => Vec::new(),
            };
            for (index, style) in styles.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("name", style.name)?;
                entry.set("kind", style.kind.name())?;
                list.set(index + 1, entry)?;
            }
            Ok(list)
        })?,
    )?;

    // A table, so adding a setting later does not change the call. Anything
    // left out keeps its default rather than being reset -- `config.lua` is
    // merged from the user's own file and may well carry only one key.
    sol.set(
        "loading",
        lua.create_function(|lua, options: mlua::Table| {
            configure(
                lua,
                "loading",
                convert::wire_doc_of(&Value::Table(options))?,
            )
        })?,
    )?;

    // Whose own size limits a floating drag is held to (#115), from
    // `sizes.lua` as it loads: `{ floating = "respect" | "ignore", ignore = {
    // app_id, ... } }`. A table so that adding a setting later does not change
    // the call; anything left out is the default, which believes every
    // application, and so is a `floating` that is not "ignore" and an `ignore`
    // that is not a list -- the words `sizes.lua` reads the layouts' settings
    // by. An entry of the list that is not a name is skipped. See
    // `ClientSizes`, and
    // `the_floating_setting_and_the_ignored_applications_reach_the_compositor`.
    sol.set(
        "client_sizes",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "client_sizes", convert::wire_section(options)?)
        })?,
    )?;

    // How a window's picture is fitted while its rectangle and its client
    // disagree about its size -- a drag ahead of its client, a glide between
    // two tiles (#233) -- and what fills the part it does not cover.
    //
    // `Option<Table>` and not `Table`, for the reason `sol.cursor_theme` below
    // spells out at length: the documented way to override the configuration is
    // one `~/.config/solium/config.lua`, and a copy written before this setting
    // existed has no `resize` key at all. A `Table` parameter would fail the
    // *whole configuration* over a setting nobody asked for -- no layouts, no
    // bindings, no windows placed.
    sol.set(
        "resize",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "resize", convert::wire_section(options)?)
        })?,
    )?;

    // What a fullscreen window covers: `{ covers = "top" | "none" }`, from
    // `init.lua` with `config.fullscreen`. An optional table for `sol.resize`'s
    // reason, and a name that is neither is named in the log and the default
    // kept, for `resize.fill`'s.
    // `the_shipped_fullscreen_setting_reaches_the_compositor`.
    sol.set(
        "fullscreen",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "fullscreen", convert::wire_section(options)?)
        })?,
    )?;

    // The pointer's XCursor theme and its size, in logical pixels.
    //
    // An optional table, the same shape as `sol.loading` above and for the
    // same reason: a setting added later does not change the call. What is
    // *different* is what an absent key means. `sol.loading` leaves the
    // compositor's default standing; here an absent key -- or no table at
    // all, which is the same answer -- means "the configuration did not say", and
    // the environment gets its turn -- `XCURSOR_THEME` and `XCURSOR_SIZE` are
    // what every other application on this machine follows, so a compositor
    // that overwrote them with defaults of its own would be the one thing on
    // screen drawing a different pointer. The order is resolved in
    // `cursor::theme::Settings::resolve`, which is where it is written down.
    //
    // Takes effect immediately, so `super+shift+r` is how a theme is tried.
    //
    // **`cursor_theme` and not `cursor`, and that name is a bug fix rather
    // than a preference.** `sol.cursor` was already taken: it is the getter
    // above that returns where the pointer *is*, and `tiling.lua` calls it on
    // every window open to decide which pane the new window splits. Registering
    // a second function under the same key silently replaced it, so every
    // `sol.on("open")` failed with "error converting Lua nil to table" and no
    // window opened in a tiled layout was ever placed. Nothing caught it:
    // `sol.set` overwrites without complaint, the compositor starts, `--check`
    // passes, and the unit tests for each of the two functions pass
    // individually because each calls its own. Only running it showed it. See
    // `two_functions_cannot_share_one_name` below, which is now the guard.
    sol.set(
        "cursor_theme",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "cursor_theme", convert::wire_section(options)?)
        })?,
    )?;

    // Which scene graph QML renders on, and how long the GPU probe may take.
    //
    // Read by the compositor from the commands the configuration loaded with,
    // before anything it asked for is applied and before any scene starts Qt
    // (`Scripts::qml`). An absent table or key means the configuration said
    // nothing. See
    // `a_configured_qml_renderer_is_read_before_qt_starts`.
    sol.set(
        "qml",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "qml", convert::wire_section(options)?)
        })?,
    )?;

    // Whether this session is announced to systemd and D-Bus activation,
    // whether that starts XDG autostart, and how long a signal to end waits
    // for a clean stop: `config.session`, handed over by `init.lua`. Read like
    // `sol.qml`, by the backend once the configuration has loaded. An absent table or key keeps the default; a value of the
    // wrong kind is named in the log and the default kept.
    // `a_configured_session_is_read_by_the_backend`.
    sol.set(
        "session",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            configure(lua, "session", convert::wire_section(options)?)
        })?,
    )?;

    // `config.run`, handed over by `init.lua` (#472): the timeout and the
    // output cap a `sol.run{}` takes when its own options leave them out.
    // Either left out, or of the wrong kind, keeps the default, named in the
    // log, as every other `sol.<setting>` here does. Kept in this Lua state,
    // where `run_read` builds each request, so a reload starts from the
    // defaults until the new configuration hands them over again.
    // `tests::sol_run_takes_the_configured_defaults_and_a_call_overrides_them`.
    sol.set(
        "run_defaults",
        lua.create_function(|lua, options: Option<Table>| {
            let defaults = RunDefaults::read(options.as_ref())?;
            lua.set_app_data(defaults);
            Ok(())
        })?,
    )?;

    // `config.files`, handed over by `init.lua` (#473): how much a
    // `sol.read` reads, how often the stores are saved, and how large a
    // store file is read. Left out, or of the wrong kind, keeps the default,
    // named in the log, as `sol.run_defaults` does. The limits are kept in
    // this Lua state, where `sol.read` and `sol.store` read them; how often
    // the stores are saved is the compositor's, as a settings section.
    // `tests::sol_read_stops_at_files_max_read`,
    // `tests::sol_files_reads_the_store_settings`.
    sol.set(
        "files",
        lua.create_function(|lua, options: Option<Table>| {
            let settings = FileSettings::read(options.as_ref())?;
            lua.set_app_data(settings);
            // How often the stores are saved is the compositor's to keep:
            // it asks for them on its own clock.
            configure(lua, "files", convert::wire_section(options)?)
        })?,
    )?;

    // How long the compositor waits for this window manager, and how often
    // it starts it again: `config.wm`, which the shipped `init.lua` hands
    // over. The compositor reads it, as every settings section, one key
    // left out or of the wrong kind keeping its default.
    // `script::tests::waits::the_wait_for_an_answer_is_the_configurations`.
    sol.set(
        "wm",
        lua.create_function(|lua, options: Option<Table>| {
            configure(lua, "wm", convert::wire_section(options)?)
        })?,
    )?;

    Ok(())
}
