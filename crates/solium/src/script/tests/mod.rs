//! The script API's tests, by area, and what they share.

use super::*;

use crate::input::devices::DeviceInfo;

use crate::input::devices::{DeviceKind, InputConfig};

use crate::present::Frame;

use smithay::utils::Logical;

use smithay::utils::Rectangle;

/// A fixture `user.lua` in a directory of its own, and the *shipped*
/// `init.lua` loaded against it.
///
/// The shipped entry point rather than a stand-in, because the thing under
/// test in every caller below is whether the file people actually run
/// honours the configuration -- a copy that requires the right modules in
/// the right order would pass while the real one did not, which is the
/// class of bug being fixed.
///
/// `None` when this machine has any Lua of its own under
/// `~/.config/solium`: that directory comes *first* on `package.path`, so
/// one file there answers `require` ahead of the fixture or the shipped
/// script it shadows, and the assertions below would then be about
/// somebody's own configuration. Any `.lua` at all rather than a list of
/// names, because the list would have to grow every time a script is added
/// and would fail open when somebody forgot. The same guard, for the same
/// reason, as `a_user_file_using_the_old_key_still_chooses_a_style`.
fn shipped_init_with_user(name: &str, user: &str) -> Option<(std::path::PathBuf, Scripts)> {
    let (directory, loaded) = shipped_init_and_user(name, user)?;
    Some((directory, loaded.expect("loading the shipped init.lua")))
}

/// [`shipped_init_with_user`], with what the load answered, for a test of
/// a `user.lua` that should be refused.
fn shipped_init_and_user(
    name: &str,
    user: &str,
) -> Option<(std::path::PathBuf, anyhow::Result<Scripts>)> {
    let own = Scripts::user_config_dir()?;
    if let Ok(entries) = std::fs::read_dir(&own)
        && entries
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().is_some_and(|kind| kind == "lua"))
    {
        return None;
    }

    let directory = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).ok()?;
    std::fs::write(directory.join("user.lua"), user).ok()?;
    // Copied rather than required in place: `Scripts::load` puts the
    // config's *own* directory on `package.path` ahead of the shipped one,
    // and that is how the fixture `user.lua` beside it gets found at all.
    let shipped = crate::assets::lua().join("init.lua");
    let config = directory.join("init.lua");
    std::fs::copy(&shipped, &config).ok()?;

    let loaded = Scripts::load(&config);
    Some((directory, loaded))
}

/// Every spawn a key produced, as `(program, args)`.
fn spawns(outcome: &Outcome) -> Vec<(String, Vec<String>)> {
    outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Spawn { program, args } => Some((program.clone(), args.clone())),
            _ => None,
        })
        .collect()
}

/// The `sol.surface` declarations named `shell`, and whether one was taken
/// away, out of a batch of commands.
fn shell_surfaces(commands: &[Command]) -> (Vec<crate::scripted::Declaration>, bool) {
    let declared = commands
        .iter()
        .filter_map(|command| match command {
            Command::Surface(declared) if declared.name == "shell" => Some((**declared).clone()),
            _ => None,
        })
        .collect();
    let removed = commands
        .iter()
        .any(|command| matches!(command, Command::SurfaceGone(name) if name == "shell"));
    (declared, removed)
}

/// The shipped configuration, then the screens arriving, as `shell` sees
/// it: what a cold start and a reload both do.
fn shell_after_monitors(name: &str, user: &str) -> Option<(Scripts, Vec<Command>)> {
    let (directory, mut scripts) = shipped_init_with_user(name, user)?;
    let mut commands = scripts.startup().commands;
    commands.extend(scripts.monitors_changed(one_screen(&[])).commands);
    let _ = std::fs::remove_dir_all(&directory);
    Some((scripts, commands))
}

/// What the child is given, and what its configuration says instead.
const ENVIRONMENT_SHELL: &str = "/solium-fixture/from-the-environment/shell.qml";

const CONFIGURED_SHELL: &str = "/solium-fixture/from-the-configuration/shell.qml";

fn input_config(outcome: Outcome) -> Option<InputConfig> {
    outcome
        .commands
        .into_iter()
        .find_map(|command| match command {
            Command::Input(config) => Some(config),
            _ => None,
        })
}

/// Every `Command::FocusMode` a run of commands carries, as `(click,
/// follow, clear_on_empty_click)`, in order.
fn focus_modes(commands: Vec<Command>) -> Vec<(Option<bool>, Option<bool>, Option<bool>)> {
    commands
        .into_iter()
        .filter_map(|command| match command {
            Command::FocusMode {
                click,
                follow,
                clear_on_empty_click,
                ..
            } => Some((click, follow, clear_on_empty_click)),
            _ => None,
        })
        .collect()
}

/// Every `Command::FocusMode`'s `after_close` a run of commands carries,
/// in order.
fn after_closes(commands: Vec<Command>) -> Vec<Option<Vec<crate::successor::Successor>>> {
    commands
        .into_iter()
        .filter_map(|command| match command {
            Command::FocusMode { after_close, .. } => Some(after_close),
            _ => None,
        })
        .collect()
}

/// A snapshot with nothing in it, for the tests that only want a call made.
fn empty_snapshot() -> Snapshot {
    Snapshot {
        locked: false,
        notifications: Default::default(),
        audio: Default::default(),
        tray: Default::default(),
        media: Default::default(),
        keyboard: crate::keymap::State::initial(),
        shortcuts_owner: None,
        windows: Vec::new(),
        monitors: Vec::new(),
        work_area: Rect::default(),
        cursor: (0.0, 0.0),
        screens: Vec::new(),
        text_input: None,
        problems: Vec::new(),
        apps: std::rc::Rc::default(),
    }
}

/// The depth and pivot of each `Present` in a batch, in order.
fn presented(commands: &[Command]) -> Vec<(f32, (f32, f32))> {
    commands
        .iter()
        .map(|command| match command {
            Command::Present { z, pivot, .. } => (*z, *pivot),
            other => panic!("expected a present command, got {other:?}"),
        })
        .collect()
}

/// Who each named selection holds, after a batch of commands.
///
/// Folded rather than indexed, because `regroup` re-declares every desk on
/// every event and a test that asserted on `commands[4]` would be pinning
/// the order of a loop over monitors.
fn membership(commands: &[Command]) -> std::collections::HashMap<String, Vec<u64>> {
    let mut out = std::collections::HashMap::new();
    for command in commands {
        if let Command::Group {
            name,
            selection: Some(selection),
            ..
        } = command
        {
            out.insert(name.clone(), selection.windows.clone());
        }
    }
    out
}

/// Where each named selection was last asked to sit, after a batch.
fn carried(commands: &[Command]) -> std::collections::HashMap<String, (f64, f64)> {
    let mut out = std::collections::HashMap::new();
    for command in commands {
        match command {
            Command::PresentGroup { name, to, .. } => {
                out.insert(name.clone(), to.offset());
            }
            Command::ClearGroup { name, .. } => {
                out.insert(name.clone(), (0.0, 0.0));
            }
            _ => {}
        }
    }
    out
}

/// One 1600x900 screen and three windows on it, for the reload test.
fn one_screen(windows: &[u64]) -> Snapshot {
    Snapshot {
        locked: false,
        notifications: Default::default(),
        audio: Default::default(),
        tray: Default::default(),
        media: Default::default(),
        keyboard: crate::keymap::State::initial(),
        shortcuts_owner: None,
        windows: windows
            .iter()
            .map(|id| WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                id: *id,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 800.0,
                    h: 600.0,
                },
                drawn: Drawn::default(),
                title: String::new(),
                focused: false,
                monitor: "test-1".to_owned(),
                // Ordinary windows: this fixture is about what a reload
                // keeps, and #72's modality plays no part in it. Spelled
                // out rather than defaulted so that a third field added to
                // `WindowInfo` fails here and is decided for this test
                // rather than silently inherited.
                modal: false,
                parent: Parentage::None,
                leaving: false,
                minimized: false,
                above: false,
                app_id: String::new(),
                min: None,
                max: None,
                cramped: false,
                shown: true,
                fullscreen: false,
                maximized: false,
                style: String::new(),
                x11_type: None,
                accepts_input: true,
                class: None,
                instance: None,
                content_type: "none",
                pid: None,
            })
            .collect(),
        monitors: vec![MonitorInfo {
            name: "test-1".to_owned(),
            area: Rect {
                x: 0.0,
                y: 0.0,
                w: 1600.0,
                h: 900.0,
            },
            whole: Rect {
                x: 0.0,
                y: 0.0,
                w: 1600.0,
                h: 900.0,
            },
            scale: 1.0,
            focused: true,
            primary: true,
            transform: "normal".to_owned(),
            off: false,
        }],
        work_area: Rect {
            x: 0.0,
            y: 0.0,
            w: 1600.0,
            h: 900.0,
        },
        cursor: (0.0, 0.0),
        screens: Vec::new(),
        text_input: None,
        problems: Vec::new(),
        apps: std::rc::Rc::default(),
    }
}

/// Every key `config.lua`'s `open_sections` accepts for one section.
///
/// Scraped from the shipped file rather than restated here: a third copy of
/// a list whose second copy is the defect under test would be the same
/// mistake with more steps.
fn accepted_keys(section: &str) -> Vec<String> {
    let path = crate::assets::lua().join("config.lua");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let Some(table) = text.split("local open_sections = {").nth(1) else {
        return Vec::new();
    };
    let Some(rest) = table.split(&format!("{section} = {{")).nth(1) else {
        return Vec::new();
    };
    // Comments stripped before anything else is looked for, because a line
    // documenting `sol.keyboard{ active = 2 }` holds both a closing brace
    // and an `=` -- so leaving them in ends the table early and drops the
    // very key the comment is about. No string in this table holds a `--`.
    let without_comments: String = rest
        .lines()
        .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
        .collect::<Vec<&str>>()
        .join("\n");
    let Some(body) = without_comments.split('}').next() else {
        return Vec::new();
    };
    let mut found: Vec<String> = body
        .split(',')
        .filter_map(|entry| entry.split_once('='))
        .filter(|(_, value)| value.trim() == "true")
        .map(|(key, _)| key.trim().to_owned())
        .collect();
    found.sort();
    found
}

/// The keys a section's parser reads out of the document it is handed.
///
/// Asked of the parser through the document's own record of what was
/// looked up in it, so this cannot be a restatement of the reads and
/// cannot drift from them however they are spelled -- `keyboard` reads
/// five of its keys through a closure and three directly, and a scan for
/// `get(` would see only three.
fn keys_read_by(name: &str) -> Vec<String> {
    doc::tests::keys_read(|| match name {
        "cursor_theme" => {
            let _ = crate::script::settings::cursor::parse(&Doc::table());
        }
        "keyboard" => {
            let _ = crate::script::settings::keyboard::parse(&Doc::table());
        }
        other => panic!("no parser named for sol.{other}"),
    })
}

fn loaded(name: &str, script: &str) -> (std::path::PathBuf, Scripts) {
    let directory = std::env::temp_dir().join(name);
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(&config, script).expect("writing the test script");
    let scripts = Scripts::load(&config).expect("loading the test script");
    (directory, scripts)
}

/// The `sol.run{}` requests among `outcome`'s commands.
fn run_requests(outcome: &Outcome) -> Vec<crate::runs::Request> {
    outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::RunRead(request) => Some(request.clone()),
            _ => None,
        })
        .collect()
}

/// Start `requests` as the compositor would, read them until each is
/// done, and tell `scripts` what was heard after every read: the status
/// each telling left, in order.
fn run_to_the_end(scripts: &mut Scripts, requests: Vec<crate::runs::Request>) -> Vec<String> {
    let mut runs = crate::runs::Runs::default();
    let count = requests.len();
    for request in requests {
        let mut process = crate::launch::command(&request.argv[0]);
        process.args(&request.argv[1..]);
        runs.start(request, process, Duration::ZERO);
    }
    let started = std::time::Instant::now();
    let mut done = 0;
    let mut statuses = Vec::new();
    while done < count {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the runs never ended"
        );
        let heard = runs.pump(Duration::ZERO);
        done += heard
            .iter()
            .filter(|each| matches!(each, crate::runs::Heard::Done { .. }))
            .count();
        if !heard.is_empty() {
            statuses.extend(scripts.runs_heard(&heard, one_screen(&[])).status);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    statuses
}

/// Play `outcome`'s timer commands into `timers` at `now`.
fn play_timers(timers: &mut crate::timers::Timers, outcome: &Outcome, now: Duration) {
    for command in &outcome.commands {
        match command {
            Command::Timer { id, after, period } => timers.add(*id, now, *after, *period),
            Command::TimerCancel(id) => timers.cancel(*id),
            _ => {}
        }
    }
}

/// Move a clock from 0 to `until` in `step`s, telling `scripts` the
/// timers due at each, as `timers::settle` does.
fn run_timers(scripts: &mut Scripts, timers: &mut crate::timers::Timers, until: u64, step: u64) {
    let mut at = 0;
    while at <= until {
        let now = Duration::from_millis(at);
        let due = timers.due(now);
        if !due.is_empty() {
            let outcome = scripts.timers_due(&due, one_screen(&[]));
            play_timers(timers, &outcome, now);
        }
        at += step;
    }
}

/// Play `outcome`'s file commands into `files`, as the compositor does.
fn play_files(files: &mut crate::files::Files, outcome: &Outcome) {
    for command in &outcome.commands {
        match command.clone() {
            Command::FileRead(request) => files.read(request),
            Command::FileWrite(request) => files.write(request),
            Command::Watch { id, path, shown } => files.watch(id, &path, shown),
            Command::WatchCancel(id) => files.cancel_watch(id),
            Command::StoreLoaded { name, text } => files.loaded_store(name, text),
            _ => {}
        }
    }
}

/// Pump `files`, telling `scripts` what was heard and playing back what
/// that asks for, until the script's global `seen` holds `count`
/// entries: those, sorted.
fn pump_files(scripts: &mut Scripts, files: &mut crate::files::Files, count: usize) -> Vec<String> {
    let started = std::time::Instant::now();
    let seen = |scripts: &Scripts| -> Vec<String> {
        let mut seen: Vec<String> = scripts
            .evaluate("return table.concat(seen, '\\n')")
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        seen.sort();
        seen
    };
    while seen(scripts).len() < count {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "only {:?}",
            seen(scripts)
        );
        let heard = files.pump();
        if !heard.is_empty() {
            let outcome = scripts.files_heard(&heard, one_screen(&[]));
            play_files(files, &outcome);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    seen(scripts)
}

/// Every `sol.act` in `outcome`, settled as done.
fn all_done(outcome: &Outcome) -> Vec<Settled> {
    outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Act { attempt, .. } => Some(Settled {
                attempt: *attempt,
                ok: true,
                reason: None,
            }),
            _ => None,
        })
        .collect()
}

/// What `tracing` logs on this thread while `run` runs.
pub(crate) fn logged_while(run: impl FnOnce()) -> String {
    #[derive(Clone, Default)]
    struct Lines(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Lines {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("the log lock was poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // A global subscriber that wants nothing, set once. With this
    // thread's subscriber the only one registered, `tracing` asks only
    // the thread that first reaches a call site whether it is wanted,
    // and caches the answer for every thread: a test on another thread
    // reaching "was stopped" first turned it off here, and `script::`
    // failed every time on two threads. With two registered, each event
    // asks the subscriber of its own thread.
    // `tests::a_stopped_listener_is_logged_with_its_file_and_line`.
    static QUIET: std::sync::Once = std::sync::Once::new();
    QUIET.call_once(|| {
        let _ =
            tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default());
    });
    let lines = Lines::default();
    let writer = lines.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let bytes = lines
        .0
        .lock()
        .map(|bytes| bytes.clone())
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

mod binding_options;
mod bindings;
mod dialogs;
mod directions;
mod dispatch;
mod input;
mod keep;
mod monitors;
mod present;
mod process;
mod rust_wm;
mod services;
mod settings;
mod shipped;
mod waits;
mod windows;

impl Scripts {
    /// Run a chunk and hand back what it returns, as text.
    ///
    /// For a test to read what a script recorded or computed, which is
    /// otherwise invisible from out here: a dispatch returns commands, and a
    /// table a script keeps for itself is not one. Test-only, and after the
    /// first `#[cfg(test)]` on purpose -- `every_event_listened_for_is_one_that_is_sent`
    /// reads this file's production half by cutting at the first one.
    pub(crate) fn evaluate(&self, chunk: &str) -> String {
        let chunk = chunk.to_owned();
        self.link.run(
            Box::new(move |wm| {
                let lua = &wm.host().lua;
                lua.set_app_data(crate::script::lua::Asker(wm.asks()));
                lua.load(&chunk)
                    .eval::<String>()
                    .unwrap_or_else(|err| format!("error: {err}"))
            }),
            &self.answers(None),
        )
    }

    /// [`Self::evaluate`], with `snapshot` as what the compositor looks like:
    /// so a chunk that asks `sol.window_at` is answered from the snapshot a
    /// handler dispatched at that instant would be handed, rather than from
    /// whichever the last dispatch left behind. For a test that asks from Lua
    /// what the compositor asks in Rust --
    /// `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws`.
    pub(crate) fn evaluate_in(&self, snapshot: Snapshot, chunk: &str) -> String {
        let changes = crate::wm::diff::Told::new().changes(&snapshot);
        let chunk = chunk.to_owned();
        self.link.run(
            Box::new(move |wm| {
                let mut mirror = solium_wm_protocol::mirror::Mirror::new();
                mirror.apply_all(changes);
                let lua = &wm.host().lua;
                lua.set_app_data(std::rc::Rc::new(mirror));
                lua.set_app_data(crate::script::lua::Asker(wm.asks()));
                lua.load(&chunk)
                    .eval::<String>()
                    .unwrap_or_else(|err| format!("error: {err}"))
            }),
            &crate::wm::events::Answers::over(snapshot),
        )
    }
}
