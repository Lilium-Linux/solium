//! **#281: Solium's own lock**, held by no client: `lock_natively`,
//! `unlock_natively`, and what every rule of `lock.rs` makes of it.

use super::*;
use crate::lock::Lock;
use crate::lock_scene::Shown;

/// Whether Solium holds the lock itself.
fn natively_locked(session: &Session) -> bool {
    session.state.lock.as_ref().is_some_and(Lock::is_native)
}

/// Whether `locked` is still owed.
fn owed(session: &Session) -> bool {
    session.state.lock.as_ref().is_some_and(Lock::pending)
}

/// A scene file of `source`, in a directory of the test's own.
fn scene_file(name: &str, source: &str) -> std::path::PathBuf {
    let directory =
        std::env::temp_dir().join(format!("solium-native-lock-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a directory for the scene");
    let path = directory.join("Lock.qml");
    std::fs::write(&path, source).expect("writing the scene");
    path
}

/// **A session scene keeps its rows through a native lock and
/// its unlock**: a scene hosted as any `sol.surface` is, with a
/// delegate for every app and every window, as a home screen
/// has, loses none of them and builds none again, through the
/// lock's entry, the lock itself and the unlock. The session's
/// scenes are not drawn behind the lock, so nothing is emptied
/// for them; emptied, every tile and its icon was built again at
/// the unlock, which stalled Illia's Surface for 270 ms over the
/// lock's exit. What is drawn over the lock reads empty models
/// of its own (`lock_auth::tests::a_lock_scene_reads_only_what_a_lock_may_show`,
/// `lock_auth::tests::the_pointer_scene_reads_only_what_a_lock_may_show`).
#[test]
fn a_session_scene_keeps_its_rows_through_a_native_lock() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut session = Session::new();
        session.app.open(&mut session.display, &mut session.state);
        session
            .state
            .lock_scenes
            .use_scene(scene_file("keeps-rows", "import QtQuick\nItem {}\n"));
        session.state.apps_scan_pending = false;
        session.state.apps = std::rc::Rc::new(vec![crate::apps::Entry {
            id: "solium-keeps-rows-test".to_owned(),
            name: "Kept".to_owned(),
            untranslated_name: "Kept".to_owned(),
            generic_name: String::new(),
            icon: String::new(),
            exec: "true".to_owned(),
            categories: Vec::new(),
            keywords: Vec::new(),
            terminal: false,
            path: None,
            source: std::path::PathBuf::from("/nonexistent/solium-keeps-rows-test.desktop"),
        }]);
        let directory = std::env::temp_dir().join(format!(
            "solium-native-lock-keeps-rows-session-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("a directory for the scene");
        let path = directory.join("Home.qml");
        std::fs::write(
            &path,
            r#"import QtQuick
            import Solium
            Item {
                id: root
                property int built: 0
                property int destroyed: 0
                readonly property int apps: Apps.count
                readonly property int windows: Windows.count
                Repeater {
                    model: Apps
                    delegate: Item {
                        Component.onCompleted: root.built += 1
                        Component.onDestruction: root.destroyed += 1
                    }
                }
                Repeater {
                    model: Windows
                    delegate: Item {
                        Component.onCompleted: root.built += 1
                        Component.onDestruction: root.destroyed += 1
                    }
                }
            }"#,
        )
        .expect("writing the scene");
        let mut home = crate::surface::ShellSurface::hosted(path, "{}", &session.output(0).name())
            .expect("the session's scene builds");
        let read = |home: &mut crate::surface::ShellSurface| {
            let scene = home.scene_for_test();
            [
                scene.get_int("apps"),
                scene.get_int("windows"),
                scene.get_int("built"),
                scene.get_int("destroyed"),
            ]
        };

        session.state.publish_models();
        let before = read(&mut home);
        assert!(
            before[0] >= 1 && before[1] >= 1,
            "the premise: an app and a window, {before:?}"
        );
        assert_eq!(
            (before[2], before[3]),
            (before[0] + before[1], 0),
            "the premise: a delegate for each"
        );

        session.state.lock_natively();
        crate::render::frame_tick(&mut session.state);
        assert_eq!(read(&mut home), before, "through the lock's entry");
        let enter = session.state.lock_scenes.settings().enter;
        session.state.clock.advance(enter);
        crate::lock_scene::settle(&mut session.state);
        session.show_all();
        crate::render::frame_tick(&mut session.state);
        assert!(natively_locked(&session), "the premise: locked");
        assert_eq!(
            read(&mut home),
            before,
            "natively locked: [apps, windows, built, destroyed]"
        );

        session.state.unlock_natively();
        crate::render::frame_tick(&mut session.state);
        assert_eq!(
            read(&mut home),
            before,
            "unlocked: [apps, windows, built, destroyed]"
        );
        session.state.unpublish_session_for_test();
        drop(home);
        let _ = std::fs::remove_dir_all(&directory);
    });
}

/// **A native lock blanks every monitor on the first frame and
/// takes input away, before any scene is ready.** Off the Qt
/// thread no scene is built, which is the moment before one is:
/// every frame built is the lock's, every monitor shows the
/// backdrop, and the keyboard is on nothing. `locked` waits for
/// every monitor, as it does for a client.
#[test]
fn a_native_lock_blanks_every_monitor_and_takes_input_before_any_scene_is_ready() {
    let mut session = Session::with_monitors(2);
    session.app.open(&mut session.display, &mut session.state);
    assert!(
        session.app.client.keyboard_on.is_some(),
        "the premise: the application has the keyboard"
    );

    session.state.lock_natively();
    session.app.pump(&mut session.display, &mut session.state);

    assert!(
        natively_locked(&session),
        "the session did not lock natively"
    );
    assert!(
        session.state.lock_frame().is_some(),
        "a frame built now is not the lock's, so it is the desktop"
    );
    for index in 0..2 {
        let output = session.output(index);
        assert_eq!(
            session.state.lock_scenes.shown_on(&output),
            Shown::Backdrop,
            "monitor {index}: no scene is ready, and the backdrop is what shows"
        );
    }
    assert!(
        session.app.client.keyboard_on.is_none(),
        "the application kept the keyboard at the native lock"
    );
    let (app, _) = session.type_key();
    assert!(
        !app,
        "a key typed at the native lock reached the application"
    );

    assert!(
        owed(&session),
        "`locked` before any monitor showed the lock"
    );
    let first = session.output(0);
    session.show(&first);
    assert!(
        owed(&session),
        "`locked` with one monitor of two showing the lock"
    );
    session.show_all();
    assert!(
        !owed(&session),
        "`locked` never came with every monitor showing it"
    );
}

/// **A scene that fails to load is stood in for on its monitor --
/// by the shipped scene, and by the fallback if that fails too --
/// and the session stays locked.**
#[test]
fn a_scene_that_fails_to_load_is_stood_in_for_and_the_session_stays_locked() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut session = Session::new();
        session.app.open(&mut session.display, &mut session.state);
        let broken = scene_file("broken", "import QtQuick\nItem { not QML at all }\n");
        session.state.lock_scenes.use_scene(broken);

        session.state.lock_natively();
        // The shipped scene that stands in comes down first.
        let enter = session.state.lock_scenes.settings().enter;
        session.state.clock.advance(enter);
        crate::lock_scene::settle(&mut session.state);
        session.show_all();
        session.app.pump(&mut session.display, &mut session.state);

        let output = session.output(0);
        assert_eq!(
            session.state.lock_scenes.shown_on(&output),
            Shown::Shipped,
            "a scene that will not load is not replaced by the shipped one"
        );
        assert!(
            natively_locked(&session),
            "a broken scene unlocked the session"
        );
        assert!(
            !owed(&session),
            "and the fallback's lock is confirmed like any"
        );
        let (app, _) = session.type_key();
        assert!(!app, "a key reached the application behind the fallback");
    });
}

/// **A client asking to lock while natively locked is refused**:
/// told `finished`, never `locked`, its surfaces never shown or
/// given the keyboard, and its unlock is `invalid_unlock` and
/// unlocks nothing.
#[test]
fn a_client_asking_to_lock_while_natively_locked_is_refused() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    session.state.lock_natively();
    session.show_all();

    let (lock, _surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    let id = wayland_client::Proxy::id(&lock);
    assert!(
        session.locker.client.finished.contains(&id),
        "a client asking to lock over the native lock was not told `finished`"
    );
    assert!(!session.locker.client.locked.contains(&id));
    assert!(natively_locked(&session), "the client took the native lock");
    assert_eq!(
        session
            .state
            .lock
            .as_ref()
            .map_or(0, |held| held.surfaces().count()),
        0,
        "the refused client's surfaces are the lock's"
    );
    assert!(
        session.locker.client.keyboard_on.is_none(),
        "the refused client was given the keyboard"
    );

    lock.unlock_and_destroy();
    let error = session
        .locker
        .pump_or_error(&mut session.display, &mut session.state);
    assert!(
        natively_locked(&session),
        "the refused client unlocked the session"
    );
    assert_eq!(
        error.map(|error| error.code),
        Some(ext_session_lock_v1::Error::InvalidUnlock as u32),
        "its unlock was not `invalid_unlock`"
    );
}

/// **An abandoned client lock can be taken over natively**, as a
/// new lock client may take it over, and a live one cannot. The
/// native lock is then confirmed and ended like its own.
#[test]
fn an_abandoned_client_lock_can_be_taken_over_natively() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let _lock = session.lock();

    session.state.lock_natively();
    assert!(
        !natively_locked(&session),
        "a native lock took the session from a lock client that is still here"
    );

    // The lock client crashes.
    let recovery = Side::connect(&mut session.display, &mut session.state);
    drop(std::mem::replace(&mut session.locker, recovery));
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.state.lock.as_ref().is_some_and(Lock::abandoned),
        "the premise: the lock is abandoned"
    );

    session.state.lock_natively();
    assert!(
        natively_locked(&session),
        "the abandoned lock was not taken over"
    );
    assert!(
        session
            .state
            .lock
            .as_ref()
            .is_some_and(|held| !held.abandoned()),
        "the native lock reads as abandoned"
    );
    assert!(owed(&session), "the new lock's `locked` is owed anew");
    session.show_all();
    assert!(!owed(&session), "and confirmed once every monitor shows it");
    let (app, _) = session.type_key();
    assert!(!app, "a key reached the application through the take-over");

    session.state.unlock_natively();
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.state.lock.is_none(),
        "the native unlock did not unlock"
    );
    assert!(
        session.app.client.keyboard_on.is_some(),
        "the application was not given the keyboard back"
    );
}

/// Calls every function `sol` has, and every one in a table it
/// has, but its private ones, with no argument, an empty table and
/// `true`, each under `pcall`; then says how many calls it made,
/// and how many of their answers held the digits typed at the
/// lock, `97531864`, as text or as a number.
/// Never a string or a number: those would start programs.
const EVERY_SOL_FUNCTION: &str = r#"
    sol.bind("super+shift+f12", function()
        local seen, calls, leaks = {}, 0, 0
        local function look(value, depth)
            if type(value) == "string" then
                if value:find("97531864", 1, true) then leaks = leaks + 1 end
            elseif type(value) == "number" then
                if value == 97531864 then leaks = leaks + 1 end
            elseif type(value) == "table" and depth < 4 then
                for key, inner in pairs(value) do
                    look(key, depth + 1)
                    look(inner, depth + 1)
                end
            end
        end
        local function answered(ok, ...)
            for index = 1, select('#', ...) do
                look((select(index, ...)), 0)
            end
        end
        local function walk(table_, depth)
            if seen[table_] or depth > 2 then return end
            seen[table_] = true
            local names = {}
            for name in pairs(table_) do
                if type(name) == "string" and name:sub(1, 1) ~= "_" then
                    names[#names + 1] = name
                end
            end
            table.sort(names)
            for _, name in ipairs(names) do
                local value = table_[name]
                if type(value) == "function" then
                    answered(pcall(value))
                    answered(pcall(value, {}))
                    answered(pcall(value, true))
                    calls = calls + 3
                elseif type(value) == "table" then
                    walk(value, depth + 1)
                end
            end
        end
        walk(sol, 0)
        -- And the peek by name, with what a gesture would give it
        -- and what it would not: at a lock it is nothing.
        local peek = type(sol.lock_peek) == "function"
        for _, value in ipairs({ 0.5, 1, 0, -1, 2, 0 / 0, 1 / 0, false }) do
            pcall(sol.lock_peek, value)
            calls = calls + 1
        end
        pcall(sol.lock_peek)
        sol.status(tostring(calls) .. " leaks=" .. tostring(leaks)
            .. (peek and " peek" or ""))
    end, { locked = true })
"#;

/// **No Lua function unlocks a native lock, or reads what is
/// typed at it**: with a PIN set and the lock taking it, and
/// digits typed, every `sol` function there is, called from a
/// binding allowed at the lock with the shipped configuration
/// loaded, leaves the session locked, checks nothing, and answers
/// nothing that holds the digits.
#[test]
fn no_lua_function_unlocks_a_native_lock() {
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
    let script = format!(
        "package.path = {shipped:?} .. \"/?.lua\"\n\
         dofile({shipped:?} .. \"/init.lua\")\n{EVERY_SOL_FUNCTION}"
    );
    let mut session = Session::new();
    session
        .state
        .start_scripts(Some(super::super::fullscreen_glides::script_at(
            "native-unlock",
            &script,
        )));
    // A PIN set, as `solium pin set` leaves it, in a directory of
    // the test's own.
    let dir = std::env::temp_dir().join(format!("solium-lua-walk-pin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let mut paths = crate::pin::Paths::in_dir(&dir);
    paths.boot_id = dir.join("boot_id");
    std::fs::write(&paths.boot_id, "boot-one\n").expect("a boot id");
    let mut pin = crate::auth::Secret::new();
    assert!(pin.push_str("1379"));
    let phc = solium_auth::pin::hash(&pin).expect("a hash");
    solium_auth::pin::write_private(&paths.pin, phc.as_bytes()).expect("the PIN");
    crate::pin::Record::after_password(crate::pin::Clocks::at(1_000), Some("boot-one".to_owned()))
        .write(&paths.state)
        .expect("the state");
    session.state.lock_auth.use_pin_for_test(paths, 1_000);

    session.state.lock_natively();
    session.show_all();
    assert_eq!(
        session.state.lock_auth.mode(),
        crate::pin::Mode::Pin,
        "the premise: the lock takes the PIN"
    );
    for c in "97531864".chars() {
        session.state.secret_edit(crate::lock_auth::Edit::Type(c));
    }
    assert_eq!(session.state.lock_auth.length(), 8);

    assert!(
        session.state.trigger("super+shift+f12"),
        "the binding did not run"
    );
    session.app.pump(&mut session.display, &mut session.state);
    let now = session.state.clock.now();
    session.state.settle(now);

    let status = std::mem::take(&mut session.state.status);
    let calls: usize = status
        .split(' ')
        .next()
        .and_then(|calls| calls.parse().ok())
        .unwrap_or_default();
    assert!(
        calls > 100,
        "the walk did not get through `sol`: {calls} calls"
    );
    assert!(
        status.ends_with(" peek"),
        "the walk did not call `sol.lock_peek`: {status:?}"
    );
    assert!(
        status.contains(" leaks=0 "),
        "a Lua function answered with the digits typed: {status:?}"
    );
    assert_eq!(
        session.state.lock_auth.length(),
        8,
        "a Lua function submitted or cleared what was typed"
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        session.state.lock_scenes.peeking().is_none(),
        "a peek is under way at the lock"
    );
    assert!(
        natively_locked(&session),
        "a Lua function unlocked the native lock"
    );
    assert!(
        !owed(&session),
        "and it is still the lock that was confirmed"
    );
}

/// **`sol.lock_now` locks, natively by default, and never
/// unlocks**: called unlocked it takes Solium's own lock, called
/// again at that lock it changes nothing, not even which lock is
/// confirmed, and with `lock.native` off and no locker named it
/// locks nothing.
#[test]
fn sol_lock_now_locks_and_never_unlocks() {
    let script = r#"
        sol.bind("super+shift+f11", function() sol.lock_now() end, { locked = true })
    "#;
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    session
        .state
        .start_scripts(Some(super::super::fullscreen_glides::script_at(
            "lock-now", script,
        )));

    assert!(session.state.trigger("super+shift+f11"));
    assert!(natively_locked(&session), "sol.lock_now did not lock");
    session.show_all();
    let frame = session.state.lock_frame();
    assert!(!owed(&session), "the premise: the lock is confirmed");

    assert!(session.state.trigger("super+shift+f11"));
    assert!(
        natively_locked(&session),
        "sol.lock_now at the lock unlocked it"
    );
    assert_eq!(
        session.state.lock_frame(),
        frame,
        "sol.lock_now at the lock took a fresh lock"
    );
    assert!(!owed(&session), "and owes `locked` again");

    session.state.unlock_natively();
    session
        .state
        .lock_scenes
        .configure(crate::lock_scene::Settings {
            native: false,
            ..crate::lock_scene::Settings::default()
        });
    assert!(session.state.trigger("super+shift+f11"));
    assert!(
        session.state.lock.is_none(),
        "locked with `lock.native` off and no locker named"
    );
}

/// **A monitor plugged in mid-lock gets the scene or the
/// fallback, never the desktop**: its frames are the lock's from
/// the hotplug on, it has a scene before its first frame, and
/// `locked` waits for it.
#[test]
fn a_monitor_plugged_in_while_natively_locked_gets_a_scene_never_the_desktop() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut session = Session::new();
        session.app.open(&mut session.display, &mut session.state);
        session.state.lock_natively();
        session.show_all();
        let first = session.output(0);
        assert_eq!(
            session.state.lock_scenes.shown_on(&first),
            Shown::Shipped,
            "the premise: the first monitor has its scene"
        );

        let plugged = session.plug();
        assert!(
            session.state.lock_frame().is_some(),
            "a frame built for the new monitor is not the lock's"
        );
        assert_ne!(
            session.state.lock_scenes.shown_on(&plugged),
            Shown::Backdrop,
            "the monitor plugged in mid-lock has no scene"
        );
        assert!(natively_locked(&session));
    });
}

/// **The entry** (#281): as Solium's own lock is taken its scenes
/// come down over the desktop for `lock.enter_ms`, like a sheet.
/// The lock is the lock from the first instant all the same.
mod entry {
    use super::*;

    /// A scene that animates nothing, which the entry brings
    /// down all the same.
    const STILL: &str = "import QtQuick\nItem {}\n";

    /// A session on `count` monitors with an application holding
    /// the keyboard, locked natively a moment ago, its scenes
    /// [`STILL`] and coming down. On the Qt thread, with Qt up.
    fn locking(name: &str, count: i32) -> Session {
        crate::qml::start().expect("Qt starts");
        let mut session = Session::with_monitors(count);
        session.app.open(&mut session.display, &mut session.state);
        assert!(
            session.app.client.keyboard_on.is_some(),
            "the premise: the application has the keyboard"
        );
        session.state.lock_scenes.use_scene(scene_file(name, STILL));
        session.state.lock_natively();
        session.app.pump(&mut session.display, &mut session.state);
        assert!(
            natively_locked(&session) && session.state.lock_scenes.entry_pending(),
            "the premise: the lock is taken, and coming down"
        );
        session
    }

    /// Whether a picture of the `index`th monitor drawn now has
    /// the desktop under its lock scene; a `capture`'s or a
    /// monitor's.
    fn desktop(session: &Session, index: usize, capture: bool) -> bool {
        let now = session.state.clock.now();
        session
            .state
            .desktop_under_lock(&session.output(index), now, capture)
    }

    /// The clock `by` on, and the loop's check of the entry.
    fn advance(session: &mut Session, by: Duration) {
        session.state.clock.advance(by);
        crate::lock_scene::settle(&mut session.state);
    }

    fn enter(session: &Session) -> Duration {
        session.state.lock_scenes.settings().enter
    }

    /// **Input is the lock's at the instant it is taken, though
    /// the desktop is still drawn**: through the entry the
    /// application has lost the keyboard and a key reaches it
    /// not, while its window is still on the screen under the
    /// scene coming down.
    #[test]
    fn input_is_the_locks_at_once_though_the_desktop_is_still_drawn() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut session = locking("entry-input", 1);
            assert!(
                desktop(&session, 0, false),
                "the desktop is not drawn under the scene coming down"
            );
            assert!(
                session.app.client.keyboard_on.is_none(),
                "the application kept the keyboard through the entry"
            );
            let (app, _) = session.type_key();
            assert!(!app, "a key typed during the entry reached the application");
            assert_eq!(
                session.state.lock_auth.length(),
                1,
                "a key typed during the entry is not the password's"
            );
            assert!(
                desktop(&session, 0, false),
                "the premise: still in the entry"
            );
        });
    }

    /// **The desktop is drawn under the scene only during
    /// `lock.enter_ms`**, on every monitor: not a moment before
    /// its end is it gone, and from its end it is never drawn
    /// again. A lock taken at once, as before sleep, never
    /// draws it.
    #[test]
    fn the_desktop_is_drawn_under_the_scene_only_during_enter_ms() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut session = locking("entry-window", 2);
            let enter = enter(&session);
            assert!(desktop(&session, 0, false) && desktop(&session, 1, false));
            // Half way, and then past the bound: the clock runs in
            // real time too, so not to the millisecond.
            advance(&mut session, enter / 2);
            assert!(
                desktop(&session, 0, false) && desktop(&session, 1, false),
                "the desktop went before `enter_ms`"
            );
            advance(&mut session, enter / 2);
            assert!(
                !desktop(&session, 0, false) && !desktop(&session, 1, false),
                "the desktop outlived the entry"
            );
            advance(&mut session, enter);
            assert!(!desktop(&session, 0, false), "the desktop came back");

            session.state.unlock_natively();
            session.state.lock_natively_at_once();
            assert!(natively_locked(&session));
            assert!(
                !session.state.lock_scenes.entry_pending()
                    && !desktop(&session, 0, false)
                    && !desktop(&session, 1, false),
                "a lock taken at once drew the desktop"
            );
        });
    }

    /// **`locked` is confirmed only after the entry**: every
    /// monitor showing a frame built during it is not the lock
    /// on every monitor, nor is one built during it and shown
    /// after; a frame built after it is.
    #[test]
    fn locked_is_confirmed_only_after_the_entry() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut session = locking("entry-locked", 1);
            session.show_all();
            assert!(
                owed(&session),
                "`locked` with the desktop still under the scene"
            );
            let during = session.state.lock_frame();
            let enter = enter(&session);
            advance(&mut session, enter);
            assert!(
                owed(&session),
                "`locked` at the entry's end, before a frame showed the lock"
            );
            let output = session.output(0);
            session.state.frame_presented(&output, during);
            assert!(
                owed(&session),
                "a frame built during the entry and shown after it counted"
            );
            session.show_all();
            assert!(!owed(&session), "`locked` never came after the entry");
        });
    }

    /// **A capture during the entry shows no desktop**: the
    /// picture a capture would be drawn from has the backdrop
    /// under the scene from the lock's first instant, and a
    /// client's capture is refused outright.
    #[test]
    fn a_capture_during_the_entry_shows_no_desktop() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut session = locking("entry-capture", 1);
            assert!(desktop(&session, 0, false), "the premise: the entry");
            assert!(
                !desktop(&session, 0, true),
                "a capture during the entry has the desktop in it"
            );
            let manager = session
                .app
                .client
                .screencopy
                .clone()
                .expect("zwlr_screencopy_manager_v1 bound");
            let output = session.app.client.output.clone().expect("wl_output bound");
            let _during = manager.capture_output(0, &output, &session.app.qh, ());
            session.app.pump(&mut session.display, &mut session.state);
            assert_eq!(
                (
                    session.app.client.captures_offered,
                    session.app.client.captures_failed
                ),
                (0, 1),
                "(offered a buffer, refused): a capture asked for during the entry"
            );
        });
    }
}

/// **The peek** (#281): a configuration's own gesture pulls the
/// lock's scenes down over the unlocked desktop with
/// `sol.lock_peek`, a picture that locks nothing, and locks from
/// there with `sol.lock_now`.
mod peek {
    use super::*;

    /// A scene that says whether it reads `"peeking"`.
    const PEEKING: &str = r#"import QtQuick
    import Solium
    Item {
        readonly property int peeking: Lock.state === "peeking" ? 1 : 0
    }"#;

    /// The pull from the top edge that docs/ricing.md ("Your own
    /// lock screen") gives, as it is printed there: the one Lua
    /// block in it that calls `sol.lock_peek`.
    fn documented_gesture() -> String {
        let docs = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/ricing.md"));
        docs.split("```lua\n")
            .skip(1)
            .filter_map(|block| block.split("```").next())
            .find(|block| block.contains("sol.lock_peek"))
            .expect("docs/ricing.md shows a gesture calling sol.lock_peek")
            .to_owned()
    }

    /// A session on `count` monitors with an application holding
    /// the keyboard, its lock scenes `source`, and `script` as its
    /// configuration. On the Qt thread, with Qt up.
    fn session(name: &str, count: i32, source: &str, script: &str) -> Session {
        crate::qml::start().expect("Qt starts");
        let mut session = Session::with_monitors(count);
        session.app.open(&mut session.display, &mut session.state);
        assert!(
            session.app.client.keyboard_on.is_some(),
            "the premise: the application has the keyboard"
        );
        session
            .state
            .lock_scenes
            .use_scene(scene_file(name, source));
        session.state.sync_lock_scenes();
        session
            .state
            .start_scripts(Some(super::super::super::fullscreen_glides::script_at(
                name, script,
            )));
        session
    }

    /// The first monitor's whole area, which the fingers are on.
    fn region(session: &Session) -> Rectangle<i32, Logical> {
        let output = session.output(0);
        session
            .state
            .space
            .output_geometry(&output)
            .expect("a monitor in the space")
    }

    /// A finger from the top edge of the first monitor, `to`
    /// of its height down, and held there.
    fn pull(session: &mut Session, to: f64) {
        let region = region(session);
        crate::synth::touch_down(&mut session.state, region, 0, (960.0, 4.0), 10_000);
        crate::synth::touch_frame(&mut session.state, region);
        for step in 1_u32..=4 {
            let y = 4.0 + (1080.0 * to - 4.0) * f64::from(step) / 4.0;
            crate::synth::touch_motion(
                &mut session.state,
                region,
                0,
                (960.0, y),
                10_000 + 40_000 * u64::from(step),
            );
            crate::synth::touch_frame(&mut session.state, region);
        }
        session.app.pump(&mut session.display, &mut session.state);
    }

    /// That finger let go.
    fn let_go(session: &mut Session) {
        let region = region(session);
        crate::synth::touch_up(&mut session.state, region, 0, 400_000);
        crate::synth::touch_frame(&mut session.state, region);
        session.app.pump(&mut session.display, &mut session.state);
    }

    /// Whether the `index`th monitor's lock scene is drawn over
    /// the unlocked desktop now.
    fn over(session: &Session, index: usize) -> bool {
        let now = session.state.clock.now();
        session
            .state
            .lock_scene_over_desktop(&session.output(index), now)
    }

    /// A frame, then the clock `by` on and the loop's check of
    /// an exit: the exit's bound is counted from its first
    /// frame, which `render::frame_tick` starts as here. Not the
    /// whole of it: its models would be left in Qt's stores,
    /// which every test on the Qt thread shares.
    fn advance(session: &mut Session, by: Duration) {
        let now = session.state.clock.now();
        session.state.lock_scenes.exit_frame(now);
        session.state.clock.advance(by);
        crate::lock_scene::settle(&mut session.state);
    }

    /// **A peek draws the lock scene over the desktop and leaves
    /// input with the session**: `sol.lock_peek` from a binding
    /// draws every monitor's scene over the desktop, and locks
    /// nothing: no lock and no frame of one, the application
    /// keeps the keyboard and hears a key typed, and the
    /// password field hears nothing.
    #[test]
    fn a_peek_draws_the_scene_over_the_desktop_and_leaves_input_with_the_session() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut session = session(
                "peek-draws",
                2,
                PEEKING,
                r#"sol.bind("super+shift+f9", function() sol.lock_peek(0.4) end)"#,
            );
            assert!(!over(&session, 0) && !over(&session, 1), "the premise");
            assert!(session.state.trigger("super+shift+f9"));
            session.app.pump(&mut session.display, &mut session.state);
            assert_eq!(session.state.lock_scenes.peeking(), Some(0.4));
            assert!(
                over(&session, 0) && over(&session, 1),
                "a monitor's scene is not drawn over the desktop in a peek"
            );
            assert!(
                session.state.lock.is_none() && session.state.lock_frame().is_none(),
                "a peek locked"
            );
            assert!(
                session.app.client.keyboard_on.is_some(),
                "the application lost the keyboard to a peek"
            );
            let (app, _) = session.type_key();
            assert!(app, "a key typed in a peek did not reach the application");
            assert_eq!(
                session.state.lock_auth.length(),
                0,
                "a key typed in a peek reached the password"
            );
            assert!(over(&session, 0), "the premise: still peeking");
        });
    }

    /// **The documented pull locks from where the finger let go**:
    /// a finger from the top edge pulls the scene down after it
    /// with nothing locked; let go past halfway, `sol.lock_now`
    /// takes the lock at once, input with it, and the entry goes
    /// on from the peek for what is left of `enter_ms`, after
    /// which, and only after which, `locked` is confirmed.
    #[test]
    fn sol_lock_now_from_a_peek_continues_from_its_progress() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let gesture = documented_gesture();
            let mut session = session("peek-lock", 1, PEEKING, &gesture);
            pull(&mut session, 0.6);
            let peek = session.state.lock_scenes.peeking().unwrap_or_default();
            assert!(
                (peek - 0.6).abs() < 0.01,
                "the scene did not follow the finger: {peek}"
            );
            assert!(over(&session, 0));
            assert!(
                session.state.lock.is_none() && session.app.client.keyboard_on.is_some(),
                "the pull locked, or took the keyboard"
            );

            let_go(&mut session);
            assert!(
                natively_locked(&session),
                "letting go past halfway did not lock"
            );
            assert!(
                session.state.lock_scenes.peeking().is_none(),
                "the peek outlived the lock"
            );
            assert!(
                session.app.client.keyboard_on.is_none(),
                "the application kept the keyboard as the lock was taken"
            );
            let (app, _) = session.type_key();
            assert!(!app, "a key reached the application at the lock");
            assert_eq!(session.state.lock_auth.length(), 1);
            assert!(session.state.lock_scenes.entry_pending());
            let now = session.state.clock.now();
            assert!(
                session
                    .state
                    .desktop_under_lock(&session.output(0), now, false),
                "the entry from a peek has no desktop under it"
            );

            let enter = session.state.lock_scenes.settings().enter;
            let left = enter.mul_f64(1.0 - peek);
            session.show_all();
            assert!(owed(&session), "`locked` during the entry from a peek");
            advance(&mut session, left / 2);
            assert!(
                session.state.lock_scenes.entry_pending(),
                "the entry from a peek ended before what was left of it"
            );
            advance(&mut session, left / 2);
            assert!(
                !session.state.lock_scenes.entry_pending(),
                "the entry from a peek lasted longer than what was left of it"
            );
            assert!(owed(&session), "`locked` before a frame showed the lock");
            session.show_all();
            assert!(!owed(&session), "`locked` never came after the entry");
        });
    }

    /// **The documented pull let go early locks nothing**: the
    /// scene goes back up through the exit, over a desktop that
    /// had its input all along, and is then drawn no more.
    #[test]
    fn a_pull_let_go_early_goes_back_up_and_locks_nothing() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let gesture = documented_gesture();
            let mut session = session("peek-early", 1, PEEKING, &gesture);
            pull(&mut session, 0.3);
            assert!(over(&session, 0), "the premise: a peek");
            let_go(&mut session);
            assert!(session.state.lock.is_none(), "letting go early locked");
            assert!(
                session.state.lock_scenes.peeking().is_none() && over(&session, 0),
                "the scene did not go back up through the exit"
            );
            let exit = session.state.lock_scenes.settings().exit;
            advance(&mut session, exit);
            assert!(!over(&session, 0), "the scene outlived its exit");
            assert!(
                session.app.client.keyboard_on.is_some(),
                "the application lost the keyboard"
            );
            let (app, _) = session.type_key();
            assert!(app);
        });
    }

    /// **A monitor plugged in during a peek gets the peeking
    /// scene**: built live, reading `"peeking"`, and drawn over
    /// the desktop like the others; when the peek ends, it goes
    /// back up with them and is drawn on no monitor after.
    #[test]
    fn a_monitor_plugged_in_during_a_peek_gets_the_peeking_scene() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut session = session(
                "peek-plug",
                1,
                PEEKING,
                r#"sol.bind("super+shift+f9", function() sol.lock_peek(0.4) end)
                   sol.bind("super+shift+f10", function() sol.lock_peek(false) end)"#,
            );
            assert!(session.state.trigger("super+shift+f9"));
            let plugged = session.plug();
            assert_eq!(
                session.state.lock_scenes.shown_on(&plugged),
                Shown::Own,
                "the monitor plugged in during a peek has no scene"
            );
            session.state.publish_models();
            let peeking = session
                .state
                .lock_scenes
                .instance_for_test(&plugged)
                .expect("its scene")
                .scene_for_test()
                .get_int("peeking");
            assert_eq!(peeking, 1, "its scene does not read \"peeking\"");
            assert!(over(&session, 0) && over(&session, 1));

            assert!(session.state.trigger("super+shift+f10"));
            assert!(over(&session, 1), "no way back up on the new monitor");
            let exit = session.state.lock_scenes.settings().exit;
            advance(&mut session, exit);
            assert!(
                !over(&session, 0) && !over(&session, 1),
                "a scene outlived the peek's exit"
            );
            assert!(session.state.lock.is_none());
        });
    }
}
