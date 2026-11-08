//! Programs, timers, files and stores, the lock, reload and quit.

use super::*;

/// **`sol.run` with a table hands its output to `done`** (#472): the
/// program's stdout, stderr, code and the rest, once, and its functions
/// are let go. The variadic form is still fire and forget.
#[test]
fn sol_run_with_a_table_hands_its_output_to_done() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-run-done",
        r#"sol.run({ "sh", "-c", "echo out; echo err >&2; exit 4" }, {
               done = function(r)
                   sol.status(r.stdout .. r.stderr .. tostring(r.code) .. " "
                       .. tostring(r.signal) .. " " .. tostring(r.timed_out) .. " "
                       .. tostring(r.truncated) .. " " .. tostring(r.error))
               end,
           })
           sol.run("true")"#,
    );
    let outcome = scripts.startup();
    assert!(
        outcome.commands.iter().any(|command| matches!(
            command,
            Command::Run { program, args } if program == "true" && args.is_empty()
        )),
        "the variadic form changed: {:?}",
        outcome.commands
    );
    let requests = run_requests(&outcome);
    let [request] = requests.as_slice() else {
        panic!("one run: {requests:?}")
    };
    assert_eq!(
        (request.timeout, request.max_output, request.lines),
        (Duration::from_secs(30), 1 << 20, false),
        "the defaults"
    );
    assert_eq!(
        run_to_the_end(&mut scripts, requests),
        ["out\nerr\n4 nil false false nil"]
    );
    assert_eq!(
        scripts.evaluate("return tostring(next(sol._runs))"),
        "nil",
        "a done run's functions were kept"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.run` tells each line, then `done`** (#472): `(line, stream)`
/// for every whole line, the last unterminated one at the end, and the
/// options it was given reach the request.
#[test]
fn sol_run_tells_lines_then_done() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-run-lines",
        r#"local seen = {}
           sol.run({ "sh", "-c", "printf 'a\nb'; echo c >&2" }, {
               timeout = 5000,
               max_output = 4096,
               cwd = "/",
               env = { SOLIUM_A = "1", SOLIUM_B = false },
               lines = function(line, stream) seen[#seen + 1] = stream .. ":" .. line end,
               done = function(r) sol.status(table.concat(seen, ",") .. "|" .. r.stdout) end,
           })"#,
    );
    let requests = run_requests(&scripts.startup());
    let [request] = requests.as_slice() else {
        panic!("one run: {requests:?}")
    };
    assert_eq!(
        (
            request.timeout,
            request.max_output,
            request.lines,
            request.cwd.as_deref()
        ),
        (Duration::from_secs(5), 4096, true, Some("/"))
    );
    assert_eq!(
        request.env,
        [
            ("SOLIUM_A".to_owned(), Some("1".to_owned())),
            ("SOLIUM_B".to_owned(), None)
        ]
    );
    let statuses = run_to_the_end(&mut scripts, requests);
    assert_eq!(
        statuses.last().map(String::as_str),
        Some("stdout:a,stderr:c,stdout:b|a\nb")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.run` takes the configured defaults, and a call overrides them**
/// (#472): `config.run`'s `timeout` and `max_output`, through
/// `sol.run_defaults`, are what a call that names neither gets; a value
/// of the wrong kind keeps the built-in default.
#[test]
fn sol_run_takes_the_configured_defaults_and_a_call_overrides_them() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-run-defaults",
        r#"sol.run({ "true" })
           sol.run_defaults({ timeout = 5000, max_output = 10 })
           sol.run({ "true" })
           sol.run({ "true" }, { timeout = 7000, max_output = 20 })
           sol.run_defaults({ timeout = 0, max_output = "lots" })
           sol.run({ "true" })"#,
    );
    let taken: Vec<(Duration, usize)> = run_requests(&scripts.startup())
        .iter()
        .map(|request| (request.timeout, request.max_output))
        .collect();
    assert_eq!(
        taken,
        [
            (Duration::from_secs(30), 1 << 20),
            (Duration::from_secs(5), 10),
            (Duration::from_secs(7), 20),
            (Duration::from_secs(30), 1 << 20),
        ]
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.run` refuses what it cannot do**: an option it does not know, a
/// timeout that is not above 0, an empty argv.
#[test]
fn sol_run_refuses_an_option_it_does_not_know() {
    let (directory, scripts) = loaded("solium-script-test-run-refuses", "");
    for (chunk, says) in [
        (
            r#"sol.run({ "true" }, { timout = 5 })"#,
            "no option `timout`",
        ),
        (r#"sol.run({ "true" }, { timeout = 0 })"#, "above 0"),
        (r#"sol.run({ "true" }, { max_output = -1 })"#, "max_output"),
        (r#"sol.run({}, {})"#, "needs a program"),
        (r#"sol.every(0, function() end)"#, "above 0"),
    ] {
        let answer = scripts.evaluate(&format!(
            "local ok, err = pcall(function() {chunk} end) return tostring(ok) .. ' ' .. tostring(err)"
        ));
        assert!(
            answer.starts_with("false ") && answer.contains(says),
            "{chunk}: {answer}"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// **Timers run in the order they fall due** (#472), and by creation
/// among those due at once; `sol.after`'s once, `sol.every`'s each
/// period.
#[test]
fn timers_run_in_the_order_they_fall_due() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-timers-order",
        r#"order = {}
           local function note(name) return function() order[#order + 1] = name end end
           sol.after(50, note("b"))
           sol.after(10, note("a"))
           sol.after(50, note("c"))
           sol.every(20, note("e"))"#,
    );
    let mut timers = crate::timers::Timers::default();
    play_timers(&mut timers, &scripts.startup(), Duration::ZERO);
    run_timers(&mut scripts, &mut timers, 60, 10);
    assert_eq!(
        scripts.evaluate("return table.concat(order, ',')"),
        "a,e,e,b,c,e"
    );
    assert_eq!(timers.len(), 1, "only the every is still set");
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A cancelled timer does not run** (#472): cancelled from a handler,
/// or from another timer due at the same moment; and cancelling one that
/// has run changes nothing.
#[test]
fn a_cancelled_timer_does_not_run() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-timers-cancel",
        r#"ran = {}
           local every = sol.every(10, function() ran[#ran + 1] = "every" end)
           local other
           sol.after(30, function() other:cancel() end)
           other = sol.after(30, function() ran[#ran + 1] = "other" end)
           local once = sol.after(5, function() ran[#ran + 1] = "once" end)
           sol.on("surface", function() every:cancel() once:cancel() end)"#,
    );
    let mut timers = crate::timers::Timers::default();
    play_timers(&mut timers, &scripts.startup(), Duration::ZERO);
    run_timers(&mut scripts, &mut timers, 20, 5);
    let outcome = scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
    play_timers(&mut timers, &outcome, Duration::from_millis(20));
    assert!(
        outcome
            .commands
            .iter()
            .all(|command| matches!(command, Command::TimerCancel(_))),
        "{:?}",
        outcome.commands
    );
    run_timers(&mut scripts, &mut timers, 100, 5);
    assert_eq!(
        scripts.evaluate("return table.concat(ran, ',')"),
        "once,every,every"
    );
    assert_eq!(timers.len(), 0);
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A timer stopped three times is cancelled** (#472): its function ran
/// past the handler deadline each time, and the compositor is told to
/// forget it.
#[test]
fn a_timer_stopped_three_times_is_cancelled() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-timers-stopped",
        "sol.every(10, function() while true do end end)",
    );
    let mut timers = crate::timers::Timers::default();
    play_timers(&mut timers, &scripts.startup(), Duration::ZERO);
    let mut cancelled = Vec::new();
    for round in 1..=3_u64 {
        let due = timers.due(Duration::from_millis(10 * round));
        assert_eq!(due.len(), 1, "round {round}");
        let outcome = scripts.timers_due(&due, one_screen(&[]));
        cancelled.push(
            outcome
                .commands
                .iter()
                .any(|command| matches!(command, Command::TimerCancel(_))),
        );
        play_timers(&mut timers, &outcome, Duration::from_millis(10 * round));
    }
    assert_eq!(cancelled, [false, false, true]);
    assert_eq!(timers.len(), 0);
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.read` hands the file to its function** (#473): a `/proc` file,
/// whose size says 0, whole; a missing one, `nil` and the system's
/// words; and the function is let go once told.
#[test]
fn sol_read_hands_the_file_to_its_function() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-read",
        r#"seen = {}
           sol.read("/proc/self/status", function(data, err)
               seen[#seen + 1] = "proc " .. tostring(data:sub(1, 5)) .. " " .. tostring(err)
           end)
           sol.read("/nonexistent-solium-file", function(data, err)
               seen[#seen + 1] = "missing " .. tostring(data) .. " " .. err
           end)"#,
    );
    let mut files = crate::files::Files::default();
    let outcome = scripts.startup();
    assert!(
        outcome.commands.iter().any(|command| matches!(
            command,
            Command::FileRead(request) if request.max == crate::files::MAX_READ
        )),
        "the default cap: {:?}",
        outcome.commands
    );
    play_files(&mut files, &outcome);
    assert_eq!(
        pump_files(&mut scripts, &mut files, 2),
        [
            "missing nil No such file or directory (os error 2)",
            "proc Name: nil"
        ]
    );
    assert_eq!(scripts.evaluate("return tostring(next(sol._files))"), "nil");
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.write` writes, and tells its function**: `nil` when the file
/// is written, the error when its directory is missing; one without a
/// function is still written, and kept nowhere.
#[test]
fn sol_write_writes_and_tells_its_function() {
    let directory = std::env::temp_dir().join("solium-script-test-write");
    let _ = std::fs::remove_dir_all(&directory);
    let (_, mut scripts) = loaded(
        "solium-script-test-write",
        &format!(
            r#"seen = {{}}
               sol.write("{dir}/a", "first", function(err) seen[#seen + 1] = "a " .. tostring(err) end)
               sol.write("{dir}/b", "quiet")
               sol.write("{dir}/no/c", "x", function(err) seen[#seen + 1] = "c " .. tostring(err) end)"#,
            dir = directory.display()
        ),
    );
    let outcome = scripts.startup();
    assert_eq!(
        scripts
            .evaluate("local n = 0 for _ in pairs(sol._files) do n = n + 1 end return tostring(n)"),
        "2",
        "a write with no function was kept"
    );
    let mut files = crate::files::Files::default();
    play_files(&mut files, &outcome);
    assert_eq!(
        pump_files(&mut scripts, &mut files, 2),
        ["a nil", "c No such file or directory (os error 2)"]
    );
    let started = std::time::Instant::now();
    while files.writing() > 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        files.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        std::fs::read_to_string(directory.join("a")).ok().as_deref(),
        Some("first")
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("b")).ok().as_deref(),
        Some("quiet")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The file functions refuse what they cannot do** (#473): a relative
/// path, a watch on a missing directory, a store's bad name.
#[test]
fn the_file_functions_refuse_what_they_cannot_do() {
    let (directory, scripts) = loaded("solium-script-test-files-refuse", "");
    for (chunk, says) in [
        (
            r#"sol.read("relative", function() end)"#,
            "neither absolute",
        ),
        (r#"sol.write("./x", "y")"#, "neither absolute"),
        (r#"sol.watch("x", function() end)"#, "neither absolute"),
        (
            r#"sol.watch("/nonexistent-solium-dir/x", function() end)"#,
            "is not a directory",
        ),
        (r#"sol.store("../up", {})"#, "not a store's name"),
        (r#"sol.store("", {})"#, "not a store's name"),
    ] {
        let answer = scripts.evaluate(&format!(
            "local ok, err = pcall(function() {chunk} end) return tostring(ok) .. ' ' .. tostring(err)"
        ));
        assert!(
            answer.starts_with("false ") && answer.contains(says),
            "{chunk}: {answer}"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A watch tells its function, and a cancelled one is not told**
/// (#473): its event carries the kind and the path as it was given, and
/// after `cancel` the compositor is told to forget it.
#[test]
fn a_cancelled_watch_is_not_told() {
    let directory = std::env::temp_dir().join("solium-script-test-watch");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a directory");
    let (_, mut scripts) = loaded(
        "solium-script-test-watch-config",
        &format!(
            r#"seen = {{}}
               watch = sol.watch("{dir}/f", function(event)
                   seen[#seen + 1] = event.kind .. " " .. event.path
               end)
               sol.on("surface", function() watch:cancel() watch:cancel() end)"#,
            dir = directory.display()
        ),
    );
    let mut files = crate::files::Files::default();
    play_files(&mut files, &scripts.startup());
    assert_eq!(files.watching(), 1);
    std::fs::write(directory.join("f"), "1").expect("writing");
    pump_files(&mut scripts, &mut files, 2);
    assert_eq!(
        scripts.evaluate("return table.concat(seen, ',')"),
        format!("created {dir}/f,changed {dir}/f", dir = directory.display())
    );
    let outcome = scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
    assert!(
        matches!(&outcome.commands[..], [Command::WatchCancel(_)]),
        "{:?}",
        outcome.commands
    );
    play_files(&mut files, &outcome);
    assert_eq!(files.watching(), 0);
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A watch whose directory goes is told `removed` and let go at once**
/// (#473): its function leaves `sol._watches` without waiting for a
/// reload.
#[test]
fn a_watch_whose_directory_goes_is_told_removed_and_let_go() {
    let directory = std::env::temp_dir().join("solium-script-test-watch-gone");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a directory");
    let (config, mut scripts) = loaded(
        "solium-script-test-watch-gone-config",
        &format!(
            r#"seen = {{}}
               sol.watch("{dir}/f", function(event) seen[#seen + 1] = event.kind end)"#,
            dir = directory.display()
        ),
    );
    let mut files = crate::files::Files::default();
    play_files(&mut files, &scripts.startup());
    std::fs::remove_dir_all(&directory).expect("removing the directory");
    assert_eq!(pump_files(&mut scripts, &mut files, 1), ["removed"]);
    let started = std::time::Instant::now();
    while scripts.evaluate("return tostring(next(sol._watches))") != "nil" {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the function was kept"
        );
        let heard = files.pump();
        if !heard.is_empty() {
            play_files(&mut files, &scripts.files_heard(&heard, one_screen(&[])));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(files.watching(), 0);
    assert_eq!(
        scripts.evaluate("return table.concat(seen, ',')"),
        "removed"
    );
    let _ = std::fs::remove_dir_all(&config);
}

/// **A store takes the file's keys and keeps the defaults it lacks**
/// (#473), inside nested tables too, takes a list whole, and is the same
/// table when asked for twice; the text it loaded is handed over so it
/// is not saved again unchanged.
#[test]
fn a_store_takes_the_files_keys_and_keeps_the_defaults_it_lacks() {
    let state = crate::files::tests::scratch("script-store-fill");
    crate::files::tests::use_state_dir(&state);
    std::fs::write(
        state.join("prefs.json"),
        r#"{"volume": 70, "ui": {"dark": true}, "recent": ["a"], "gone": 1}"#,
    )
    .expect("writing");
    let (directory, mut scripts) = loaded(
        "solium-script-test-store-fill",
        r#"local s = sol.store("prefs", { volume = 50, muted = false,
               ui = { dark = false, scale = 2 }, recent = { "x", "y", "z" } })
           same = sol.store("prefs") == s
           shown = string.format("%d %s %s %d %d %s %d", s.volume, tostring(s.muted),
               tostring(s.ui.dark), s.ui.scale, #s.recent, s.recent[1], s.gone)"#,
    );
    assert_eq!(scripts.evaluate("return tostring(same)"), "true");
    assert_eq!(scripts.evaluate("return shown"), "70 false true 2 1 a 1");
    let outcome = scripts.startup();
    assert!(
        outcome.commands.iter().any(|command| matches!(
            command,
            Command::StoreLoaded { name, text }
                if name == "prefs" && text == r#"{"gone":1,"recent":["a"],"ui":{"dark":true},"volume":70}"#
        )),
        "{:?}",
        outcome.commands
    );
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&state);
}

/// **A store survives a reload as the data it saves** (#473): carried
/// as its plain data, not read from the file again, with a function in
/// it left out; and one the new configuration does not ask for is still
/// carried, for a later one that does.
#[test]
fn a_store_survives_a_reload_as_the_data_it_saves() {
    let state = crate::files::tests::scratch("script-store-reload");
    crate::files::tests::use_state_dir(&state);
    let (directory, scripts) = loaded(
        "solium-script-test-store-reload",
        r#"local s = sol.store("prefs", { volume = 50 })
           s.volume = 65
           s.run = function() end
           sol.store("other", {}).kept = "yes""#,
    );
    assert_eq!(
        scripts
            .stores()
            .iter()
            .map(|(name, json)| format!("{name}={}", json.render()))
            .collect::<Vec<_>>(),
        [r#"other={"kept":"yes"}"#, r#"prefs={"volume":65}"#]
    );
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"local s = sol.store("prefs", { volume = 50 })
           shown = tostring(s.volume) .. " " .. tostring(s.run)"#,
    )
    .expect("writing");
    let scripts = Scripts::load_carrying(&config, scripts.kept()).expect("reloading");
    assert_eq!(scripts.evaluate("return shown"), "65 nil");
    std::fs::write(&config, r#"shown = sol.store("other", {}).kept"#).expect("writing");
    let scripts = Scripts::load_carrying(&config, scripts.kept()).expect("reloading");
    assert_eq!(scripts.evaluate("return shown"), "yes");
    assert!(
        std::fs::read_dir(&state)
            .map(|mut dir| dir.next().is_none())
            .unwrap_or(true),
        "a reload read or wrote the state directory itself"
    );
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&state);
}
