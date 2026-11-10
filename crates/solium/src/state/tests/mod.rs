//! The tests of `state`: its rules asked directly (which monitor a rectangle is on, what is on
//! stage, which chrome or client a point belongs to, where a menu goes, what a grab may do), and
//! `real_client` and `drag_icon`, which stand up a display and drive it from a real client.

use super::*;

/// Two monitors side by side, 1920 wide each, as `space.output_geometry`
/// would report them.
fn two_monitors() -> [Rectangle<i32, Logical>; 2] {
    [
        Rectangle::new((0, 0).into(), (1920, 1080).into()),
        Rectangle::new((1920, 0).into(), (1920, 1080).into()),
    ]
}

fn at(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

/// **The cull `render::prepare` runs before it captures anything.**
///
/// A hidden workspace is not unmapped -- it is drawn a screen away -- so
/// "is any of this rectangle on a monitor" is the question that separates
/// a window worth capturing from one nothing will ever draw. Getting it
/// wrong in the cheap direction costs a permanent offscreen pass per
/// hidden window; getting it wrong in the other direction stops a visible
/// window being captured, which is a blank corner. So both directions are
/// asserted, and each line names an implementation it rules out.
#[test]
fn a_pane_is_on_a_monitor_only_if_some_monitor_covers_part_of_it() {
    let screens = two_monitors();

    assert!(
        anywhere_on(at(100, 100, 800, 600), screens),
        "a window in the middle of the first monitor is on it"
    );
    // Rules out `all(..)` in place of `any(..)`: this one touches only the
    // second screen, and with two monitors mapped that is the common case.
    assert!(
        anywhere_on(at(2000, 100, 800, 600), screens),
        "and one on the second monitor is on that"
    );
    assert!(
        anywhere_on(at(1800, 100, 400, 600), screens),
        "a window dragged across the bezel is on both"
    );

    // The case the cull exists for: a workspace hidden by being parked one
    // screen to the left of the desk. Rules out `|_| true`.
    assert!(
        !anywhere_on(at(-1920, 0, 1920, 1080), screens),
        "a workspace parked a screen away is on no monitor, which is how a \
         workspace switch hides one"
    );
    assert!(
        !anywhere_on(at(0, -2000, 800, 600), screens),
        "and so is one parked above the desk"
    );

    // Exclusive, matching `render::elements`. Rules out
    // `overlaps_or_touches`, which differs from `overlaps` only here.
    assert!(
        !anywhere_on(at(-800, 0, 800, 1080), screens),
        "a window whose right edge is exactly the monitor's left edge has \
         no pixel on it"
    );

    // Rules out a constant `true`, and covers the moment between a monitor
    // going away and the session noticing.
    assert!(!anywhere_on(at(100, 100, 800, 600), []));
}

/// **A reload reads the effect folders again**: the configuration's
/// `reload_from` leaves a changed effect pending for the next `prepare`.
#[test]
fn a_reload_reads_the_effect_folders_again() {
    let directory = crate::effect::host::tests::scratch("state-reload");
    let entry = directory.join("init.lua");
    std::fs::write(&entry, "sol.pane('none')\n").expect("writing the test script");
    let effects = directory.join("effects");
    let frag = "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n";
    let dir = crate::effect::host::tests::folder(
        &effects,
        "a",
        "return { api = 1, frag = 'effect.frag' }",
        &[("effect.frag", frag)],
    );
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        Some(effects.clone()),
        effects.join("none"),
    ));
    // No shipped effects, so no shipped `client_blur` to draw either.
    state.effect_settings.client_blur = None;
    state.effects.want("rules", ["a".to_owned()]);
    let v1 = state
        .effects
        .pending_hash("a")
        .expect("the premise: a cold load is pending until prepare");
    std::fs::write(
        dir.join("effect.frag"),
        "vec4 sol_effect(vec2 uv) { return vec4(1.0); }\n",
    )
    .expect("v2");
    state.reload_from(&entry);
    assert_ne!(
        state.effects.pending_hash("a"),
        Some(v1),
        "the reload did not read the folder again"
    );
    assert!(state.effects.pending_hash("a").is_some());
    let _ = std::fs::remove_dir_all(directory);
}

/// `sol.effects(<lua>)` through the compositor's `Command::Effects` arm, as
/// the shipped `init.lua` hands `config.effects` over.
fn apply_effects_lua(state: &mut Solium, lua: &str) {
    let options = tree(lua);
    let rules = options
        .field("rules")
        .map_or_else(|| Ok(Vec::new()), crate::effect::rules::parse);
    let settings = crate::effect::settings::parse(&options);
    state.apply_effects_set(rules, settings);
}

/// **`effects.sandbox` reaches the host and the styles' reader**: the caps
/// one `sol.effects` gives are what every effect's Lua and every style's
/// `effects.lua` are made with, and what it gives is kept for its readers.
#[test]
fn effects_sandbox_reaches_the_host_and_the_styles_reader() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    apply_effects_lua(
        &mut state,
        "{ sandbox = { load_ms = 300, memory_mib = 32 }, limits = { params = 12 } }",
    );
    let caps = crate::effect::settings::Caps {
        load: std::time::Duration::from_millis(300),
        memory: 32 << 20,
    };
    assert_eq!(state.effects.caps(), caps);
    let styles = crate::style::caps();
    assert_eq!(state.effect_settings.limits.params, 12);
    // Back to the defaults, which every other test reads styles under.
    apply_effects_lua(&mut state, "{}");
    assert_eq!(styles, caps, "the styles' reader kept its own caps");
    assert_eq!(
        crate::style::caps(),
        crate::effect::settings::Caps::default()
    );
}

/// **A refused setting keeps everything as it was**: a key out of its bounds
/// is a problem under `settings`, and neither the caps nor the rules of the
/// same `sol.effects` are taken; a set that parses clears it.
#[test]
fn a_refused_effects_setting_keeps_everything_as_it_was() {
    let place = crate::effect::host::tests::scratch("state-refused-setting");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let (_display, mut state) = state_with_effects_in(&place);
    let rule = "{ { match = '*', part = 'client', slot = 'behind', effect = 'tint' } }";
    apply_effects_lua(
        &mut state,
        &format!("{{ rules = {rule}, sandbox = {{ load_ms = 1 }} }}"),
    );
    assert!(
        state.rules.is_empty(),
        "the rules of a refused set were taken"
    );
    assert_eq!(
        state.effects.caps(),
        crate::effect::settings::Caps::default()
    );
    let said = |state: &Solium| {
        state
            .effects
            .problems()
            .iter()
            .filter(|problem| problem.effect == "settings")
            .map(|problem| problem.message.clone())
            .collect::<Vec<_>>()
    };
    assert!(
        said(&state)
            .iter()
            .any(|message| message.contains("effects.sandbox.load_ms")),
        "{:?}",
        state.effects.problems()
    );
    apply_effects_lua(&mut state, &format!("{{ rules = {rule} }}"));
    assert!(!state.rules.is_empty(), "{:?}", state.effects.problems());
    assert!(said(&state).is_empty(), "{:?}", said(&state));
    let _ = std::fs::remove_dir_all(place);
}

/// **A reload that no longer calls `sol.effects` drops a refused set's
/// problem**: the configuration that refused it is gone, so what it said
/// goes with it, as the new configuration says it again or not.
#[test]
fn a_reload_drops_a_refused_settings_problem() {
    let directory = crate::effect::host::tests::scratch("state-settings-reload");
    let entry = directory.join("init.lua");
    std::fs::write(&entry, "sol.pane('none')\n").expect("writing the test script");
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let settings = |state: &Solium| {
        state
            .effects
            .problems()
            .iter()
            .filter(|problem| problem.effect == "settings")
            .count()
    };
    apply_effects_lua(&mut state, "{ sandbox = { load_ms = 1 } }");
    assert_eq!(settings(&state), 1, "the premise: the set was refused");
    state.reload_from(&entry);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(
        settings(&state),
        0,
        "a configuration that no longer calls sol.effects kept its problem: {:?}",
        state.effects.problems()
    );
}

/// **`sol.startup` configures the first frame's gate, and a refusal
/// keeps what ran**: `max_ms = 500` lets the frame through at 500 ms; a
/// reload saying `max_ms = -1` is a problem under `startup`, naming the
/// key, and the gate keeps 500; a reload that mends it drops the problem.
#[test]
fn sol_startup_configures_the_gate_and_a_refusal_keeps_what_ran() {
    use std::time::Duration;
    let directory = crate::effect::host::tests::scratch("state-startup");
    let entry = directory.join("init.lua");
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let ms = Duration::from_millis;
    let said = |state: &Solium| {
        state
            .effects
            .problems()
            .iter()
            .filter(|problem| problem.effect == "startup")
            .map(|problem| problem.message.clone())
            .collect::<Vec<_>>()
    };
    let write = |lua: &str| std::fs::write(&entry, lua).expect("writing the test script");
    write("sol.pane('none')\nsol.startup({ first_frame = { max_ms = 500, on_reload = true } })\n");
    state.reload_from(&entry);
    assert!(said(&state).is_empty(), "{:?}", said(&state));
    assert!(state.warm.holding(ms(1000), false));
    assert!(state.warm.holding(ms(1499), false));
    assert!(
        !state.warm.holding(ms(1500), false),
        "max_ms = 500 not obeyed"
    );
    write("sol.pane('none')\nsol.startup({ first_frame = { max_ms = -1 } })\n");
    state.reload_from(&entry);
    let refused = said(&state);
    assert!(
        refused.len() == 1 && refused[0].contains("max_ms"),
        "{refused:?}"
    );
    assert!(
        state.warm.holding(ms(2000), false),
        "the refusal dropped on_reload"
    );
    assert!(
        !state.warm.holding(ms(2500), false),
        "the refusal dropped max_ms = 500"
    );
    write("sol.pane('none')\nsol.startup({ first_frame = { max_ms = 500 } })\n");
    state.reload_from(&entry);
    let _ = std::fs::remove_dir_all(directory);
    assert!(said(&state).is_empty(), "{:?}", said(&state));
}

/// **An injected wait holds the first frame for its passes**
/// (`SOLIUM_WARM_INJECT`, the control of `dev/effects-check.sh xray`'s
/// first frames): with nothing pending the effects are ready, yet the first
/// frame is not for the first passes injected, each held by
/// `render::hold_first_frame` as `prepare` asks it, and is ready from then
/// on, released as ready, not at the cap. With none injected, the first
/// pass draws (spec §8.4).
#[test]
fn an_injected_wait_holds_the_first_frame_for_its_passes() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    assert!(state.effects_ready(), "the premise: nothing pending");
    state.warm.inject(2);
    for pass in 0..2 {
        assert!(
            !state.first_frame_ready(),
            "pass {pass}: ready while injected"
        );
        assert!(
            crate::render::hold_first_frame(&mut state),
            "pass {pass}: not held"
        );
    }
    assert!(state.first_frame_ready(), "not ready after its passes");
    assert!(
        !crate::render::hold_first_frame(&mut state),
        "held past its injected passes"
    );
    assert!(!state.warm.capped(), "released at the cap, not as ready");
    let mut plain = Solium::new(display.handle());
    assert!(
        !crate::render::hold_first_frame(&mut plain),
        "held with nothing injected and nothing pending"
    );
}

/// **A reload arms the first frame's gate again only with `on_reload`**:
/// released at its first ready pass, a reload with `on_reload = false`
/// draws at once, and one with `on_reload = true` holds until ready, the
/// new configuration's `on_reload` the one obeyed.
#[test]
fn a_reload_arms_the_first_frames_gate_only_with_on_reload() {
    use std::time::Duration;
    let directory = crate::effect::host::tests::scratch("state-startup-reload");
    let entry = directory.join("init.lua");
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let ms = Duration::from_millis;
    let write = |on: bool| {
        std::fs::write(
            &entry,
            format!("sol.pane('none')\nsol.startup({{ first_frame = {{ on_reload = {on} }} }})\n"),
        )
        .expect("writing the test script")
    };
    write(false);
    state.reload_from(&entry);
    assert!(!state.warm.holding(ms(0), true), "the premise: released");
    state.reload_from(&entry);
    assert!(
        !state.warm.holding(ms(100), false),
        "on_reload = false held after a reload"
    );
    write(true);
    state.reload_from(&entry);
    let _ = std::fs::remove_dir_all(directory);
    assert!(
        state.warm.holding(ms(200), false),
        "a reload with on_reload = true did not arm the gate"
    );
}

/// **A refused set with broken rules says both**: its settings' error and
/// every rule's, as each alone would.
#[test]
fn a_refused_set_with_broken_rules_says_both() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    apply_effects_lua(
        &mut state,
        "{ rules = { { match = '*', part = 'regoin:titlebar', slot = 'behind', effect = false } }, sandbox = { load_ms = 1 } }",
    );
    let said = |effect: &str| {
        state
            .effects
            .problems()
            .iter()
            .any(|problem| problem.effect == effect)
    };
    assert!(said("settings"), "{:?}", state.effects.problems());
    assert!(
        said("rules"),
        "the rules' errors were dropped: {:?}",
        state.effects.problems()
    );
}

/// **A broken configuration reload is a problem**, so the overlay of the
/// configuration still running lists it, a second failure replaces it rather
/// than adding to it, and a reload that works clears it.
#[test]
fn a_failed_reload_is_a_problem_until_one_succeeds() {
    let directory = crate::effect::host::tests::scratch("state-config-problem");
    let entry = directory.join("init.lua");
    std::fs::write(&entry, "sol.pane('none')\n").expect("writing");
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let config = |state: &Solium| -> Vec<crate::effect::host::Problem> {
        state
            .effects
            .problems()
            .iter()
            .filter(|each| each.effect == "config")
            .cloned()
            .collect()
    };
    std::fs::write(&entry, "sol.pane('none')\nthis is not lua\n").expect("breaking it");
    state.reload_from(&entry);
    let problems = config(&state);
    let [problem] = problems.as_slice() else {
        panic!("one config problem: {problems:?}");
    };
    assert_eq!(problem.line, Some(2), "{problem:?}");
    assert_eq!(problem.file, entry, "{problem:?}");
    std::fs::write(&entry, "sol.pane('none')\n\nthis is not lua either\n").expect("again");
    state.reload_from(&entry);
    let lines: Vec<Option<u32>> = config(&state).iter().map(|each| each.line).collect();
    assert_eq!(lines, [Some(3)], "the second failure replaces the first");
    std::fs::write(&entry, "sol.pane('none')\n").expect("mending it");
    state.reload_from(&entry);
    assert!(
        state
            .effects
            .problems()
            .iter()
            .all(|each| each.effect != "config")
    );
    let _ = std::fs::remove_dir_all(directory);
}

/// **The scripts hear of changed problems once, after the frame**: `settle`
/// fires `problems` when they changed since it last told them, and not again
/// until they change.
#[test]
fn settle_tells_the_scripts_once_per_change_of_the_problems() {
    let directory = crate::effect::host::tests::scratch("state-problems-told");
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        "local n = 0\nsol.on('problems', function() n = n + 1; sol.status(tostring(n)) end)\n",
    )
    .expect("writing");
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.start_scripts(Some(
        crate::script::Scripts::load(&entry).expect("loading the test script"),
    ));
    let mut told = Vec::new();
    state.settle(state.clock.now());
    told.push(state.status.clone());
    state
        .effects
        .push_problem(crate::effect::host::config_problem("no line"));
    state.settle(state.clock.now());
    told.push(state.status.clone());
    state.settle(state.clock.now());
    told.push(state.status.clone());
    state.effects.clear_problems_of("config");
    state.settle(state.clock.now());
    told.push(state.status.clone());
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(told, ["", "1", "1", "2"]);
}

/// **Answers are applied in the order they came**: one the compositor
/// stopped waiting for is applied before the next, and by the event loop as
/// soon as it comes, with nothing else to turn the loop.
#[test]
fn a_late_answer_is_applied_when_it_comes_and_before_the_next() {
    use crate::script::Outcome;
    use crate::wm::link::tests::Hand;
    use solium_wm_protocol::Command;

    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let mut event_loop: smithay::reexports::calloop::EventLoop<'static, Solium> =
        smithay::reexports::calloop::EventLoop::try_new().expect("an event loop");
    state.wm_watch = Some(crate::wm::Watch::new(event_loop.handle(), |state| state));
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    scripts.waits.ask = std::time::Duration::from_millis(5);
    state.start_scripts(Some(scripts));

    state.trigger_relayout();
    assert_eq!(hand.batch(), 1);
    assert_eq!(
        state.status, "",
        "unanswered, and the compositor has gone on"
    );
    hand.answer(1, false, vec![Command::Status("late".to_owned())]);
    event_loop
        .dispatch(Some(std::time::Duration::from_secs(5)), &mut state)
        .expect("dispatching");
    assert_eq!(state.status, "late", "applied when it came");

    state.trigger_relayout();
    assert_eq!(hand.batch(), 2);
    hand.answer(2, false, vec![Command::Status("late again".to_owned())]);
    if let Some(scripts) = state.scripts.as_mut() {
        scripts.hear_ready();
    }
    state.apply(Outcome {
        status: Some("the next".to_owned()),
        ..Outcome::default()
    });
    assert_eq!(state.status, "the next", "after the one before it");
    assert!(
        state
            .scripts
            .as_ref()
            .is_some_and(|scripts| scripts.late.is_empty())
    );
}

/// **A framed pane's `style` is the configured style's name, the default's
/// when none is named, and a bare pane's is `"none"`** (Ruling 15): a rule on
/// `style = "top"` reaches a window framed by the default style as it does
/// one framed by `pane = "top"`, and never a fullscreen one.
#[test]
fn a_framed_panes_style_is_the_configured_one_or_the_default_and_a_bare_ones_is_none() {
    assert_eq!(style_fact(true, Some("frosted")), "frosted");
    assert_eq!(style_fact(true, None), "top");
    assert_eq!(style_fact(false, Some("top")), "none");
    assert_eq!(style_fact(false, None), "none");
}

/// Rules from Lua source, read the way `sol.effects` reads them (as
/// `effect::rules::tests::rules` does).
fn tree(lua: &str) -> crate::script::Doc {
    let state = mlua::Lua::new();
    let value: mlua::Value = state.load(lua).eval().expect("the test's Lua");
    crate::effect::lua_doc::doc_of_plain(&value)
        .expect("readable")
        .expect("a value")
}

type TestDisplay = smithay::reexports::wayland_server::Display<Solium>;

/// A display, and a state whose effects are looked for in `place` alone.
fn state_with_effects_in(place: &std::path::Path) -> (TestDisplay, Solium) {
    let display = TestDisplay::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        Some(place.to_path_buf()),
        place.join("none"),
    ));
    // No shipped effects, so no shipped `client_blur` to draw either.
    state.effect_settings.client_blur = None;
    (display, state)
}

const TINT: &str =
    "return { api = 1, inputs = { 'self' }, params = { amount = { 0.1 } }, frag = 'effect.frag' }";
const TINT_FRAG: &str = "vec4 sol_effect(vec2 uv) { return sol_tex(uv) * p_amount; }\n";

/// The user's newest bound plan's first step, as a test reads it.
fn first_step(state: &Solium) -> Option<&solium_effects::stage::Step> {
    state
        .chains
        .first_plan_for_test(crate::effect::rules::Origin::User)
        .and_then(|plan| plan.steps.first())
}

/// **A program run in the background opens no window** (#319): `sol.run`
/// starts it as `sol.spawn` would, and it runs, but no loading window is
/// opened for it to wait in. (`sol.spawn` opens one by building its QML scene,
/// which a plain test thread may not do once Qt has started elsewhere in the
/// process, so it is not the control here; the test fails with `run` routed
/// through `spawn`.)
#[test]
fn a_program_run_in_the_background_opens_no_window() {
    let display = TestDisplay::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let marker = std::env::temp_dir().join(format!("solium-run-test-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    state.apply(crate::script::Outcome {
        commands: vec![crate::script::Command::Run {
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), format!("touch '{}'", marker.display())],
        }],
        ..Default::default()
    });
    assert_eq!(
        state.panes.len(),
        0,
        "a window was opened for a program run in the background"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the program never ran"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let _ = std::fs::remove_file(&marker);
}

/// A state running `init.lua` as `source` reads, from a directory of its own.
fn state_running(name: &str, source: &str) -> (TestDisplay, Solium, std::path::PathBuf) {
    let directory = crate::effect::host::tests::scratch(name);
    let entry = directory.join("init.lua");
    std::fs::write(&entry, source).expect("writing");
    let display = TestDisplay::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.socket_name = format!("wayland-{name}");
    state.start_scripts(Some(
        crate::script::Scripts::load(&entry).expect("loading the test script"),
    ));
    (display, state, directory)
}

/// **A run and a timer reach Lua through the loop's own checks** (#472):
/// `sol.run{}`'s child is started as the compositor starts every program,
/// read by `runs::settle`, and its `done` sets the status; a timer set from
/// it falls due on the compositor's clock and `timers::settle` runs it.
#[test]
fn a_run_and_a_timer_reach_lua_through_the_loop() {
    let (_display, mut state, directory) = state_running(
        "state-run-and-timer",
        r#"sol.run({ "sh", "-c", "echo \"$WAYLAND_DISPLAY\"; ls /proc/self/fd | wc -l" }, {
               done = function(r)
                   sol.status("ran " .. r.stdout)
                   sol.after(1000, function() sol.status("timer") end)
               end,
           })"#,
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !state.status.starts_with("ran") {
        assert!(std::time::Instant::now() < deadline, "the run never ended");
        crate::runs::settle(&mut state);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Its own display, and stdin, stdout and stderr only (and the directory
    // `ls` reads, which is its own).
    assert_eq!(
        state.status,
        format!("ran {}\n4\n", state.socket_name),
        "not the child every program Solium starts is"
    );
    assert!(state.runs.is_idle());
    crate::timers::settle(&mut state);
    assert!(state.status.starts_with("ran"), "the timer ran early");
    state.clock.advance(std::time::Duration::from_millis(1000));
    crate::timers::settle(&mut state);
    assert_eq!(state.status, "timer");
    let _ = std::fs::remove_dir_all(directory);
}

/// **A reload kills the runs and cancels the timers** (#472): the old
/// configuration's child is killed and reaped, and neither its `done` nor
/// its timer runs into the new configuration.
#[test]
fn a_reload_kills_the_runs_and_cancels_the_timers() {
    let (_display, mut state, directory) = state_running(
        "state-reload-runs",
        r#"sol.run({ "sh", "-c", "sleep 30" }, { done = function() sol.status("old done") end })
           sol.every(10, function() sol.status("old timer") end)"#,
    );
    assert_eq!(
        (state.runs.running(), state.timers.len()),
        (1, 1),
        "the premise: a run and a timer"
    );
    let entry = directory.join("init.lua");
    std::fs::write(&entry, "sol.status('new')").expect("writing");
    state.reload_from(&entry);
    assert_eq!((state.runs.running(), state.timers.len()), (0, 0));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while state.runs.dying() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the killed run was never reaped"
        );
        crate::runs::settle(&mut state);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    state.clock.advance(std::time::Duration::from_millis(100));
    crate::runs::settle(&mut state);
    crate::timers::settle(&mut state);
    assert_eq!(state.status, "new", "the old configuration's functions ran");
    let _ = std::fs::remove_dir_all(directory);
}

/// Settle the files at `now` until no write is running.
fn files_written(state: &mut Solium, now: std::time::Duration) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    crate::files::settle_at(state, now);
    while state.files.writing() > 0 {
        assert!(std::time::Instant::now() < deadline, "a write never ended");
        std::thread::sleep(std::time::Duration::from_millis(5));
        crate::files::settle_at(state, now);
    }
}

/// Run `chunk` in `state`'s configuration, between dispatches.
fn lua(state: &Solium, chunk: &str) -> String {
    state.scripts.as_ref().expect("the scripts").evaluate(chunk)
}

/// **A store is saved at most once a second, and survives a fresh start**
/// (#473): the first look saves it, a change within the second waits for
/// the second to pass, an unchanged store is not saved again, the
/// compositor's end saves the last change, and a fresh `Scripts` reads it
/// back. Driven by the clock the loop passes, not by waiting.
#[test]
fn a_store_is_saved_at_most_once_a_second_and_survives_a_fresh_start() {
    use std::time::Duration;
    let stored = crate::files::tests::scratch("state-store-throttle");
    crate::files::tests::use_state_dir(&stored);
    let source = r#"prefs = sol.store("prefs", { volume = 50 })"#;
    let (_display, mut state, directory) = state_running("state-store-throttle", source);
    let file = stored.join("prefs.json");
    let saved = || std::fs::read_to_string(&file).unwrap_or_default();
    files_written(&mut state, Duration::ZERO);
    assert_eq!(
        saved(),
        "{\"volume\":50}\n",
        "the first look saves the defaults"
    );
    lua(&state, "prefs.volume = 60 return ''");
    files_written(&mut state, Duration::from_millis(999));
    assert_eq!(saved(), "{\"volume\":50}\n", "saved within the second");
    crate::files::settle_at(&mut state, Duration::from_millis(1000));
    assert_eq!(state.files.writing(), 1, "not saved once the second passed");
    files_written(&mut state, Duration::from_millis(1000));
    assert_eq!(saved(), "{\"volume\":60}\n");
    crate::files::settle_at(&mut state, Duration::from_millis(2000));
    assert_eq!(state.files.writing(), 0, "saved again unchanged");
    lua(&state, "prefs.volume = 70 return ''");
    crate::files::finish(&mut state);
    assert_eq!(saved(), "{\"volume\":70}\n", "the end did not save it");
    let fresh = crate::script::Scripts::load(&directory.join("init.lua")).expect("loading again");
    assert_eq!(fresh.evaluate("return tostring(prefs.volume)"), "70");
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(stored);
}

/// **A configured `files.store_interval` is the one used** (#473): with
/// 5000, a change is not saved a second later, and is at five.
#[test]
fn a_configured_store_interval_is_used() {
    use std::time::Duration;
    let stored = crate::files::tests::scratch("state-store-interval");
    crate::files::tests::use_state_dir(&stored);
    let (_display, mut state, directory) = state_running(
        "state-store-interval",
        r#"sol.files({ store_interval = 5000 })
           prefs = sol.store("prefs", { volume = 50 })"#,
    );
    let file = stored.join("prefs.json");
    let saved = || std::fs::read_to_string(&file).unwrap_or_default();
    files_written(&mut state, Duration::ZERO);
    assert_eq!(saved(), "{\"volume\":50}\n");
    lua(&state, "prefs.volume = 60 return ''");
    for at in [1000, 4999] {
        crate::files::settle_at(&mut state, Duration::from_millis(at));
        assert_eq!(state.files.writing(), 0, "saved at {at} ms");
    }
    crate::files::settle_at(&mut state, Duration::from_millis(5000));
    assert_eq!(state.files.writing(), 1, "not saved at 5000 ms");
    files_written(&mut state, Duration::from_millis(5000));
    assert_eq!(saved(), "{\"volume\":60}\n");
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(stored);
}

/// **A reload ends the watches and keeps the store** (#473): the store is
/// saved before the reload and handed to the new configuration as it was,
/// and the old watch's function is not called again.
#[test]
fn a_reload_ends_the_watches_and_keeps_the_store() {
    use std::time::Duration;
    let stored = crate::files::tests::scratch("state-reload-files");
    crate::files::tests::use_state_dir(&stored);
    let watched = crate::files::tests::scratch("state-reload-watched");
    let (_display, mut state, directory) = state_running(
        "state-reload-files",
        &format!(
            r#"prefs = sol.store("prefs", {{ n = 1 }})
               prefs.n = 2
               sol.watch("{dir}/w", function(event) sol.status("old " .. event.kind) end)"#,
            dir = watched.display()
        ),
    );
    assert_eq!(state.files.watching(), 1, "the premise: a watch");
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        r#"prefs = sol.store("prefs", { n = 1 }) sol.status("new " .. prefs.n)"#,
    )
    .expect("writing");
    state.reload_from(&entry);
    assert_eq!(state.files.watching(), 0);
    assert_eq!(state.status, "new 2");
    files_written(&mut state, Duration::ZERO);
    assert_eq!(
        std::fs::read_to_string(stored.join("prefs.json"))
            .ok()
            .as_deref(),
        Some("{\"n\":2}\n"),
        "not saved before the reload"
    );
    std::fs::write(watched.join("w"), "x").expect("writing the watched file");
    for _ in 0..20 {
        crate::files::settle_at(&mut state, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(state.status, "new 2", "the old watch's function ran");
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(stored);
    let _ = std::fs::remove_dir_all(watched);
}

/// **A `client_blur` that cannot bind costs only its own slots** (#78): with
/// no `blur` anywhere, a rule of the user's still binds and runs, and the one
/// problem listed is `client_blur`'s.
#[test]
fn a_client_blur_that_cannot_bind_costs_only_its_own_slots() {
    let place = crate::effect::host::tests::scratch("state-client-blur-unbound");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let (_display, mut state) = state_with_effects_in(&place);
    state.effect_settings.client_blur = Some(crate::script::Doc::Text("blur".to_owned()));
    let rules = crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "replace", effect = { "tint", amount = 0.2 } } }"#,
    ))
    .expect("parses");
    state.apply_effects(Ok(rules));
    assert_eq!(state.rules.user().len(), 1, "the user's rule was refused");
    let problems: Vec<String> = state
        .effects
        .problems()
        .iter()
        .map(|problem| problem.effect.clone())
        .collect();
    assert_eq!(problems, ["client_blur"], "{:?}", state.effects.problems());
}

/// **A broken rule keeps the rules that ran**, and the error is a problem;
/// **a rule naming an effect nobody ships is refused** the same way.
#[test]
fn a_broken_rule_keeps_the_rules_that_ran() {
    let place = crate::effect::host::tests::scratch("state-rules");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let (_display, mut state) = state_with_effects_in(&place);
    let good = crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "replace", effect = { "tint", amount = 0.2 } } }"#,
    ))
    .expect("parses");
    state.apply_effects(Ok(good));
    assert!(!state.rules.is_empty());
    let nobody = crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "replace", effect = "nobody" } }"#,
    ))
    .expect("parses");
    state.apply_effects(Ok(nobody));
    assert_eq!(
        state.rules.effects(),
        vec!["tint".to_owned()],
        "the rules that ran were replaced by a broken set"
    );
    assert!(
        state
            .effects
            .problems()
            .iter()
            .any(|each| each.effect == "rules" && each.message.contains("nobody")),
        "{:?}",
        state.effects.problems()
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **A rule whose effect fails its checks is named at the effect's own file
/// and line**, once, and **a set that mends it leaves nothing on the
/// overlay**: the problem of an effect no longer wanted goes with it.
#[test]
fn a_rule_whose_effect_is_broken_is_named_at_the_frags_line_until_mended() {
    let place = crate::effect::host::tests::scratch("state-rules-typo");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    crate::effect::host::tests::folder(
        &place,
        "typo",
        "return { api = 1, inputs = { 'self' }, params = { offset = { 1 } }, frag = 'down.frag' }",
        &[(
            "down.frag",
            "vec4 sol_effect(vec2 uv) {\n    return sol_tex(uv) * p_ofset;\n}\n",
        )],
    );
    let (_display, mut state) = state_with_effects_in(&place);
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "behind", effect = "typo" } }"#,
    ))
    .expect("parses")));
    assert!(state.rules.is_empty());
    let problems = state.effects.problems().to_vec();
    let [problem] = problems.as_slice() else {
        panic!("one problem: {problems:?}");
    };
    assert_eq!(
        (problem.effect.as_str(), problem.file.clone(), problem.line),
        ("rules", place.join("typo/down.frag"), Some(2)),
        "{problem:?}"
    );
    assert!(problem.message.contains("rule 1"), "{problem:?}");
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "behind", effect = "tint" } }"#,
    ))
    .expect("parses")));
    assert!(!state.rules.is_empty());
    assert_eq!(
        state.effects.problems(),
        &[],
        "a mended set left a problem behind"
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **A rule reading the backdrop binds**, from xray (T2, Task 44) or, with
/// `source = "live"`, from everything beneath its part (#209).
#[test]
fn a_blur_rule_binds_from_xray_or_live() {
    let place = crate::effect::host::tests::scratch("state-xray");
    crate::effect::host::tests::folder(
        &place,
        "soft",
        "return { api = 1, inputs = { 'backdrop' }, frag = 'effect.frag' }",
        &[(
            "effect.frag",
            "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
        )],
    );
    let (_display, mut state) = state_with_effects_in(&place);
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "behind", effect = { "soft", source = "live" } } }"#,
    ))
    .expect("parses")));
    assert!(!state.rules.is_empty(), "{:?}", state.effects.problems());
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "behind", effect = "soft" } }"#,
    ))
    .expect("parses")));
    assert!(!state.rules.is_empty(), "{:?}", state.effects.problems());
    let _ = std::fs::remove_dir_all(place);
}

/// **The probe's first answer rebinds the rules after the frame** (Ruling
/// 11): a rule bound at config load, before any probe, keeps its `rgba16f`
/// rung; the probe says no; the next settle rebinds, and the rung is gone.
#[test]
fn the_formats_probe_rebinds_the_rules_after_the_frame() {
    let place = crate::effect::host::tests::scratch("state-formats");
    let frag = "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n";
    crate::effect::host::tests::folder(
        &place,
        "fine",
        "return { api = 1, inputs = { 'self' }, params = { field = { 1, int = true } }, fallback = { { field = 0 } },
        stages = function(p) if p.field == 1 then return { { 'pass', 'a.frag', format = 'rgba16f' } } end return { { 'pass', 'a.frag' } } end }",
        &[("a.frag", frag)],
    );
    let (_display, mut state) = state_with_effects_in(&place);
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "replace", effect = { "fine", source = "self" } } }"#,
    ))
    .expect("parses")));
    let format = |state: &Solium| first_step(state).map(|step| step.format);
    assert_eq!(
        format(&state),
        Some(solium_effects::stage::Format::Rgba16f),
        "the premise: unknown formats keep the configured rung"
    );
    crate::render::note_formats(&mut state, crate::pool::Formats { rgba16f: false });
    assert!(state.rebind);
    let _ = state.settle(state.clock.now());
    assert_eq!(
        format(&state),
        Some(solium_effects::stage::Format::Rgba8),
        "the rule was not rebound after the probe"
    );
    assert!(!state.rebind);
    let _ = std::fs::remove_dir_all(place);
}

/// **`settle` rebuilds an effect its budget stopped, after the frame**
/// (Ruling 4), so the next pane's `mesh` call runs on a fresh state.
#[test]
fn settle_rebuilds_an_effect_its_budget_stopped() {
    let place = crate::effect::host::tests::scratch("state-revive");
    crate::effect::host::tests::folder(&place, "spin", crate::effect::geometry::tests::SPIN, &[]);
    let (_display, mut state) = state_with_effects_in(&place);
    state.effects.want("present", ["spin".to_owned()]);
    let stopped = state.effects.effect("spin").expect("current");
    stopped
        .sandbox()
        .lua()
        .globals()
        .set("spin", true)
        .expect("set");
    let ask = crate::effect::geometry::tests::ask();
    assert!(crate::effect::geometry::mesh(stopped.sandbox(), &ask, 1, 1).is_err());
    let _ = state.settle(state.clock.now());
    let fresh = state.effects.effect("spin").expect("still current");
    assert!(
        !fresh.sandbox().poisoned(),
        "settle did not rebuild the stopped effect"
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **A reload binds the rules against the folders it read**, so an effect
/// changed on disk is the version its rule runs: here a param's default.
#[test]
fn a_reload_binds_the_rules_against_the_folders_it_read() {
    let directory = crate::effect::host::tests::scratch("state-reload-rules");
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        "sol.pane('none')\nsol.effects({ rules = { { match = '*', part = 'client', slot = 'replace', effect = 'tint' } } })\n",
    )
    .expect("writing the test script");
    let effects = directory.join("effects");
    let dir =
        crate::effect::host::tests::folder(&effects, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let (_display, mut state) = state_with_effects_in(&effects);
    let uniforms = |state: &Solium| first_step(state).map(|step| step.uniforms.clone());
    state.reload_from(&entry);
    let before = uniforms(&state).expect("the premise: the rule is bound");
    std::fs::write(dir.join("effect.lua"), TINT.replace("0.1", "0.3")).expect("v2");
    state.reload_from(&entry);
    assert_ne!(
        uniforms(&state),
        Some(before),
        "the rule was bound against the folder as it was before the reload"
    );
    let _ = std::fs::remove_dir_all(directory);
}

/// **A rule set holds the programs it runs, and a set replacing it gives
/// them back**: what one set bound is not held for the rest of the session.
#[test]
fn a_replaced_rule_set_holds_only_its_own_programs() {
    let place = crate::effect::host::tests::scratch("state-rules-held");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    crate::effect::host::tests::folder(
        &place,
        "plain",
        "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
        &[(
            "effect.frag",
            "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
        )],
    );
    let (_display, mut state) = state_with_effects_in(&place);
    let key = |state: &Solium| first_step(state).map(|step| step.key).expect("bound");
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "replace", effect = "tint" } }"#,
    ))
    .expect("parses")));
    let tint = key(&state);
    assert!(state.effects.held_for_test().contains(&tint));
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "replace", effect = "plain" } }"#,
    ))
    .expect("parses")));
    let plain = key(&state);
    assert_ne!(tint, plain);
    assert_eq!(
        state
            .effects
            .held_for_test()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![plain],
        "the replaced set's program is still held"
    );
    let _ = std::fs::remove_dir_all(place);
}

/// The user's rule `index` as the running set keyed it.
fn running_key(state: &Solium, index: u32) -> crate::effect::rules::RuleKey {
    crate::effect::rules::RuleKey {
        origin: crate::effect::rules::Origin::User,
        index,
        generation: state.rules_generation,
    }
}

/// **A rule set re-sent with new params keeps its generation** (#305): the
/// same rules, only a param's value changed, so the slot a client's chain
/// filled is the one it keeps, capture and targets with it, and the chain
/// is the new one, its params' hash moved so it runs again. A configuration
/// that animates a param by re-sending its rules each step recaptured every
/// window each step before.
#[test]
fn a_rule_set_resent_with_new_params_keeps_its_generation() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    let place = crate::effect::host::tests::scratch("state-rules-resent");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let (_display, mut state) = state_with_effects_in(&place);
    let rules = |amount: &str| {
        crate::effect::rules::parse(&tree(&format!(
            r#"{{ {{ match = "*", part = "client", slot = "replace", effect = {{ "tint", amount = {amount} }} }} }}"#
        )))
        .expect("parses")
    };
    state.apply_effects(Ok(rules("0.1")));
    let key = running_key(&state, 0);
    let hash = state.chains.get(key).expect("bound").params_hash;
    let client = Owner::Pane(crate::pane::PaneId::from_raw(1), PaneSlot::Client);
    let slot = state.store.slot_mut(&client, Slot::Replace, key).id.clone();
    state.apply_effects(Ok(rules("0.5")));
    assert_eq!(
        running_key(&state, 0),
        key,
        "a param-only re-send started a new generation"
    );
    assert_ne!(
        state.chains.get(key).expect("still bound").params_hash,
        hash,
        "the chain kept the old params"
    );
    assert_eq!(
        state
            .store
            .slot_mut(&client, Slot::Replace, running_key(&state, 0))
            .id,
        slot,
        "the slot started afresh"
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **A param that picks other passes starts the slots afresh** (#305), as
/// does another effect, and a rebind after the formats probe however little
/// changed: a slot's capture and targets were made for the passes it ran.
#[test]
fn a_param_that_picks_other_passes_starts_the_slots_afresh() {
    let place = crate::effect::host::tests::scratch("state-rules-resent-afresh");
    crate::effect::host::tests::folder(
        &place,
        "pick",
        "return { api = 1, inputs = { 'self' }, params = { which = { 0, int = true } },
            stages = function(p) if p.which == 1 then return { { 'pass', 'b.frag' } } end return { { 'pass', 'a.frag' } } end }",
        &[
            ("a.frag", "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n"),
            ("b.frag", "vec4 sol_effect(vec2 uv) { return sol_tex(uv) * 0.5; }\n"),
        ],
    );
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let (_display, mut state) = state_with_effects_in(&place);
    let apply = |state: &mut Solium, effect: &str| {
        state.apply_effects(Ok(crate::effect::rules::parse(&tree(&format!(
            r#"{{ {{ match = "*", part = "client", slot = "replace", effect = {effect} }} }}"#
        )))
        .expect("parses")));
        state.rules_generation
    };
    let first = apply(&mut state, r#"{ "pick", which = 0 }"#);
    let other_passes = apply(&mut state, r#"{ "pick", which = 1 }"#);
    assert_ne!(
        other_passes, first,
        "a param that picks another pass kept the slots"
    );
    let other_effect = apply(&mut state, r#"{ "tint", amount = 0.1 }"#);
    assert_ne!(other_effect, other_passes, "another effect kept the slots");
    state.rebind_effects();
    assert_ne!(
        state.rules_generation, other_effect,
        "a rebind after the probe kept the slots"
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **A refused rule set leaves no compile asked for**: neither the program a
/// rule of it bound that nothing had asked for (a param picking another
/// `.frag`) nor the one an effect only it named would be compiled, so
/// nothing reaches the GPU for a set that never ran.
#[test]
fn a_refused_rule_set_leaves_no_compile_asked_for() {
    let place = crate::effect::host::tests::scratch("state-rules-refused-asks");
    crate::effect::host::tests::folder(
        &place,
        "pick",
        "return { api = 1, inputs = { 'self' }, params = { which = { 0, int = true } },
            stages = function(p) if p.which == 1 then return { { 'pass', 'b.frag' } } end return { { 'pass', 'a.frag' } } end }",
        &[
            ("a.frag", "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n"),
            ("b.frag", "vec4 sol_effect(vec2 uv) { return sol_tex(uv) * 0.5; }\n"),
        ],
    );
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let (_display, mut state) = state_with_effects_in(&place);
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{ { match = "*", part = "client", slot = "replace", effect = "pick" } }"#,
    ))
    .expect("parses")));
    let held = state.effects.held_for_test().clone();
    let asked = state.effects.asked_for_test();
    assert_eq!(asked.len(), 1, "the premise: a.frag is asked for");
    state.apply_effects(Ok(crate::effect::rules::parse(&tree(
        r#"{
            { match = "*", part = "client", slot = "replace", effect = { "pick", which = 1 } },
            { match = "*", part = "client", slot = "behind", effect = "tint" },
            { match = "*", part = "client", slot = "front", effect = "nobody" },
        }"#,
    ))
    .expect("parses")));
    assert_eq!(
        state.rules.effects(),
        vec!["pick".to_owned()],
        "the premise: the set was refused"
    );
    assert_eq!(state.effects.held_for_test(), &held);
    assert_eq!(
        state.effects.asked_for_test(),
        asked,
        "a refused set left a compile asked for"
    );
    let _ = std::fs::remove_dir_all(place);
}

/// Whether this run's environment has chosen the pane style already, so a
/// test that names one stands down rather than asserting about another.
fn the_environment_names_a_style() -> bool {
    ["SOLIUM_PANE", "SOLIUM_DECORATION", "SOLIUM_QML_TITLEBAR"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

/// A pane style folder `frosted` in `place`, with `effects.lua` as given and
/// a manifest nothing here builds.
fn style_with_rules(place: &std::path::Path, effects_lua: &str) -> std::path::PathBuf {
    let style = place.join("frosted");
    std::fs::create_dir_all(&style).expect("a style folder");
    std::fs::write(
        style.join("Pane.qml"),
        "import Solium\nPaneStyle { Layer { depth: \"frame\"; name: \"bar\" } }\n",
    )
    .expect("writing the manifest");
    std::fs::write(style.join("effects.lua"), effects_lua).expect("writing effects.lua");
    style
}

/// The style's newest bound plan's first step's program, as a test reads it.
fn style_step(state: &Solium) -> Option<u64> {
    state
        .chains
        .first_plan_for_test(crate::effect::rules::Origin::Style)
        .and_then(|plan| plan.steps.first())
        .map(|step| step.key)
}

/// **A style's rules are bound when it is applied, and a broken
/// `effects.lua` is on the overlay** (Ruling 15): its effects are wanted and
/// its rules bound at config load, not when a frame first resolves one; a
/// broken edit lists its line, and the rules that ran stay bound. Applied
/// again unchanged it tells the overlay nothing, and mended it leaves
/// nothing there.
#[test]
fn a_styles_rules_are_bound_when_it_is_applied_and_its_problems_are_on_the_overlay() {
    let place = crate::effect::host::tests::scratch("state-style-rules");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let style = style_with_rules(
        &place,
        "return { { match = '*', part = 'client', slot = 'front', effect = { 'tint', source = 'self' } } }",
    );
    let (_display, mut state) = state_with_effects_in(&place);
    state.apply_style_rules_from(Some(&style));
    assert!(
        state.effects.has_pending("tint"),
        "the style's effect is not wanted when the style is applied"
    );
    assert!(
        state
            .chains
            .first_plan_for_test(crate::effect::rules::Origin::Style)
            .is_some(),
        "the style's rule is not bound when the style is applied"
    );
    std::fs::write(
        style.join("effects.lua"),
        "return {\n  {\n    part = = 'client' } }",
    )
    .expect("breaking it");
    state.apply_style_rules_from(Some(&style));
    assert!(
        state
            .effects
            .problems()
            .iter()
            .any(|each| each.effect.starts_with("style:") && each.line == Some(3)),
        "{:?}",
        state.effects.problems()
    );
    assert!(
        state
            .chains
            .first_plan_for_test(crate::effect::rules::Origin::Style)
            .is_some(),
        "the rules that ran went with the broken edit"
    );
    let told = state.effects.problems_generation();
    state.apply_style_rules_from(Some(&style));
    assert_eq!(
        state.effects.problems_generation(),
        told,
        "applied again unchanged, the overlay was told of a change"
    );
    std::fs::write(style.join("effects.lua"), "return {}").expect("mending it");
    state.apply_style_rules_from(Some(&style));
    assert!(
        !state
            .effects
            .problems()
            .iter()
            .any(|each| each.effect.starts_with("style:")),
        "{:?}",
        state.effects.problems()
    );
    assert!(
        state
            .chains
            .first_plan_for_test(crate::effect::rules::Origin::Style)
            .is_none(),
        "a rule the mended file no longer has is still bound"
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **The shipped glass style binds liquid glass behind the pane and the
/// shade in front of it**: its `effects.lua` has two rules, the `glass`
/// effect behind every pane but a fullscreen one (T2, reading the backdrop),
/// and the `shade` in front of every such pane without the keyboard (T0,
/// reading only the pane's shape), fading in and out over 240 ms on
/// `outCubic`; both bind, and the style's effects are wanted.
#[test]
fn the_shipped_glass_style_binds_liquid_glass_behind_the_pane() {
    use crate::effect::rules::{Fade, Fill, Part, Slot, Tier};
    let style = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/panes/glass"));
    let read = crate::style::rules_of(style);
    assert!(read.problems.is_empty(), "{:?}", read.problems);
    let [glass, shade] = &read.rules[..] else {
        panic!("two rules: {:?}", read.rules)
    };
    assert_eq!((&glass.part, glass.slot), (&Part::Pane, Slot::Behind));
    assert_eq!(glass.matches.fullscreen, Some(false), "{:?}", glass.matches);
    assert!(
        matches!(&glass.fill, Fill::Chain(links) if links.len() == 1 && links[0].effect == "glass"),
        "{:?}",
        glass.fill
    );
    assert_eq!(glass.fade, None);
    assert_eq!((&shade.part, shade.slot), (&Part::Pane, Slot::Front));
    assert_eq!(
        (shade.matches.fullscreen, shade.matches.focused),
        (Some(false), Some(false)),
        "{:?}",
        shade.matches
    );
    assert!(
        matches!(&shade.fill, Fill::Chain(links) if links.len() == 1 && links[0].effect == "shade"),
        "{:?}",
        shade.fill
    );
    assert_eq!(
        shade.fade,
        Some(Fade {
            duration: std::time::Duration::from_millis(240),
            curve: solium_animation::Curve::OutCubic,
        })
    );

    let display = TestDisplay::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        None,
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/effects")).to_path_buf(),
    ));
    state.apply_style_rules_from(Some(style));
    for effect in ["glass", "shade"] {
        assert!(
            state.effects.has_pending(effect),
            "the style's {effect} is not wanted"
        );
    }
    assert!(
        !state
            .effects
            .problems()
            .iter()
            .any(|each| each.effect.starts_with("style:")),
        "{:?}",
        state.effects.problems()
    );
    let tiers: Vec<(u32, Tier)> = state
        .chains
        .all_for_test(crate::effect::rules::Origin::Style)
        .into_iter()
        .map(|(key, chain)| (key.index, chain.tier))
        .collect();
    assert_eq!(tiers, [(0, Tier::Xray), (1, Tier::Generated)], "{tiers:?}");
}

/// **A style's rule that cannot bind is a problem at its `effects.lua`, and
/// the others run** (Ruling 15): where your own set is taken whole, a
/// style's rules are bound one by one, so one naming an effect nobody ships
/// costs only its own slot.
#[test]
fn a_style_rule_that_cannot_bind_is_a_problem_and_the_others_run() {
    let place = crate::effect::host::tests::scratch("state-style-rule-unbound");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    let style = style_with_rules(
        &place,
        "return {
            { match = '*', part = 'client', slot = 'front', effect = { 'tint', source = 'self' } },
            { match = '*', part = 'client', slot = 'behind', effect = 'nobody' },
        }",
    );
    let (_display, mut state) = state_with_effects_in(&place);
    state.apply_style_rules_from(Some(&style));
    assert!(
        style_step(&state).is_some(),
        "the style's rule that binds did not run beside the one that does not"
    );
    assert!(
        state
            .effects
            .problems()
            .iter()
            .any(|each| each.effect == "style:frosted"
                && each.message.contains("rule 2")
                && each.file == style.join("effects.lua")),
        "{:?}",
        state.effects.problems()
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **A style's rules hold the programs they run, and a style replacing them
/// gives them back**, as your own set's do: what one style bound is not held
/// for the rest of the session.
#[test]
fn a_replaced_style_holds_only_its_own_programs() {
    let place = crate::effect::host::tests::scratch("state-style-held");
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    crate::effect::host::tests::folder(
        &place,
        "plain",
        "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
        &[(
            "effect.frag",
            "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
        )],
    );
    let style = style_with_rules(
        &place,
        "return { { match = '*', part = 'client', slot = 'front', effect = { 'tint', source = 'self' } } }",
    );
    let (_display, mut state) = state_with_effects_in(&place);
    state.apply_style_rules_from(Some(&style));
    let tint = style_step(&state).expect("bound");
    assert!(
        state.effects.held_for_test().contains(&tint),
        "the style's program is not held"
    );
    std::fs::write(
        style.join("effects.lua"),
        "return { { match = '*', part = 'client', slot = 'front', effect = { 'plain', source = 'self' } } }",
    )
    .expect("editing it");
    state.apply_style_rules_from(Some(&style));
    let plain = style_step(&state).expect("bound");
    assert_eq!(
        state
            .effects
            .held_for_test()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![plain],
        "the replaced style's program is still held"
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **`sol.pane` binds its style's rules, and a reload binds them again**
/// (Ruling 15): naming a style applies its `effects.lua` there and then, and
/// a reload reads the file again even when the configuration does not name
/// the style again.
#[test]
fn sol_pane_binds_the_styles_rules_and_a_reload_binds_them_again() {
    if the_environment_names_a_style() {
        return;
    }
    let directory = crate::effect::host::tests::scratch("state-style-reload");
    let effects = directory.join("effects");
    crate::effect::host::tests::folder(&effects, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    crate::effect::host::tests::folder(
        &effects,
        "plain",
        "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
        &[(
            "effect.frag",
            "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
        )],
    );
    let style = style_with_rules(
        &directory,
        "return { { match = '*', part = 'client', slot = 'front', effect = { 'tint', source = 'self' } } }",
    );
    let entry = directory.join("init.lua");
    std::fs::write(&entry, format!("sol.pane('{}')\n", style.display()))
        .expect("writing the test script");
    let (_display, mut state) = state_with_effects_in(&effects);
    state.reload_from(&entry);
    let tint = style_step(&state).expect("sol.pane did not bind its style's rules");
    std::fs::write(
        style.join("effects.lua"),
        "return { { match = '*', part = 'client', slot = 'front', effect = { 'plain', source = 'self' } } }",
    )
    .expect("editing it");
    std::fs::write(&entry, "-- the style stays as it was\n").expect("writing the test script");
    state.reload_from(&entry);
    let plain = style_step(&state).expect("bound");
    assert_ne!(
        plain, tint,
        "the reload did not bind the style's rules again"
    );
    let _ = std::fs::remove_dir_all(directory);
}

/// **The probe's first answer rebinds the style's rules too, and one this
/// GPU cannot run leaves its slot empty** (Rulings 11, 15): a style rule
/// bound before the probe keeps an effect that only draws into `rgba16f`;
/// the probe says no; after the frame its chain is gone, and said.
#[test]
fn the_formats_probe_rebinds_the_styles_rules_too() {
    if the_environment_names_a_style() {
        return;
    }
    let place = crate::effect::host::tests::scratch("state-style-formats");
    crate::effect::host::tests::folder(
        &place,
        "wide",
        "return { api = 1, inputs = { 'self' }, stages = { { 'pass', 'a.frag', format = 'rgba16f' } } }",
        &[(
            "a.frag",
            "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
        )],
    );
    let style = style_with_rules(
        &place,
        "return { { match = '*', part = 'client', slot = 'replace', effect = { 'wide', source = 'self' } } }",
    );
    let (_display, mut state) = state_with_effects_in(&place);
    state
        .decorations
        .set_style(&mut state.panes, Some(style.display().to_string()));
    state.apply_style_rules();
    assert!(
        style_step(&state).is_some(),
        "the premise: unknown formats keep the rgba16f plan"
    );
    crate::render::note_formats(&mut state, crate::pool::Formats { rgba16f: false });
    let _ = state.settle(state.clock.now());
    assert!(
        style_step(&state).is_none(),
        "a style rule this GPU cannot run is still bound after the probe"
    );
    assert!(
        state
            .effects
            .problems()
            .iter()
            .any(|each| each.effect == "style:frosted"),
        "{:?}",
        state.effects.problems()
    );
    let _ = std::fs::remove_dir_all(place);
}

/// **A deform at rest is aimed at nothing**, so a window brought back from a
/// genie with `sol.present(id, {})` is not captured and warped on every frame
/// for good (#140).
#[test]
fn a_deform_at_rest_is_aimed_at_nothing() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    // A `Rect` anchor never reads the pane, so any pane serves.
    let pane = state.panes.open(Pane::loading(
        "aimed",
        None,
        Rectangle::new((100, 100).into(), (400, 300).into()),
        std::path::PathBuf::new(),
        None,
        state.clock.now(),
    ));
    let genie = |progress| crate::present::Deform {
        effect: crate::present::Geometry {
            progress,
            ..crate::present::Geometry::for_test(crate::effect::settings::PresentFailed::Flat)
        },
        anchor: crate::present::Anchor::Rect(crate::present::logical((10.0, 10.0), (120.0, 24.0))),
    };
    assert!(
        state.aimed_at_for(pane, Some(genie(0.0))).is_none(),
        "a genie at rest still aims"
    );
    assert!(
        state.aimed_at_for(pane, Some(genie(0.5))).is_some(),
        "a genie under way must aim"
    );
}

/// **`automatic = false` leaves nothing watched** (#223): the enforcement
/// point for "never reloads" is that the loop source which would otherwise
/// wake for a change is never armed in the first place, not merely that the
/// quiet period never elapses. No Qt thread needed: `configure_autoreload`
/// touches only the watch and the stored settings. See
/// `crate::autoreload::tests` for the directory-resolution and debounce
/// logic this builds on.
#[test]
fn automatic_false_leaves_nothing_watched() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());

    state.configure_autoreload(crate::autoreload::Settings {
        automatic: false,
        quiet_ms: 300,
    });
    assert!(
        !state.autoreload_watcher.is_watching(),
        "automatic = false must not watch anything, whatever directories exist on this machine"
    );
}

/// **Turning `automatic` off clears a deadline already pending** (#223
/// review): the gap between a change being noted and the timer that would
/// fire for it actually running is real -- nothing in `tty.rs`/`winit.rs`
/// can cancel a `calloop` timer once armed -- so `configure_autoreload`
/// resetting the debounce the moment settings change is what keeps a stale
/// deadline from answering `due` later, after `automatic` is back on with no
/// new change behind it. The timer callback's own re-check of `automatic`
/// (`crate::autoreload::decide_timer_outcome`) is the other half, covered by
/// `autoreload::tests::off_drops_the_timer_without_reloading_even_when_due`.
#[test]
fn turning_automatic_off_clears_a_pending_deadline() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());

    state.configure_autoreload(crate::autoreload::Settings {
        automatic: true,
        quiet_ms: 300,
    });
    let now = state.clock.now();
    state
        .autoreload_debounce
        .note(now, std::time::Duration::from_millis(300));

    state.configure_autoreload(crate::autoreload::Settings {
        automatic: false,
        quiet_ms: 300,
    });

    assert!(
        !state
            .autoreload_debounce
            .due(now + std::time::Duration::from_millis(300)),
        "a deadline noted before automatic turned off must not still be pending after"
    );
}

/// **A loading scene set outside every watched root is warned about, once**
/// (#223 review): the gap `autoreload`'s module doc names -- a plain
/// absolute path, with no `SOLIUM_LOADING` naming it instead to fold its
/// directory into `watch_roots` through `override_roots`.
#[test]
fn warns_once_for_a_loading_scene_set_outside_every_watched_root() {
    if std::env::var_os("SOLIUM_LOADING").is_some() {
        // The environment has already chosen a loading scene for this
        // process; this test is not the one to say anything about that.
        return;
    }
    let dir = std::env::temp_dir().join(format!(
        "solium-state-test-unwatched-loading-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    let scene = dir.join("Loading.qml");
    std::fs::write(&scene, "").expect("writing a throwaway scene file");

    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.autoreload_settings.automatic = true;
    state.loading.scene = Some(scene.to_str().expect("utf-8 temp path").to_string());

    state.warn_about_unwatched_configured_paths();
    assert!(
        state.autoreload_unwatched_warned.contains(&scene),
        "a loading scene outside the watched roots and the shipped assets must be warned about"
    );

    state.warn_about_unwatched_configured_paths();
    assert_eq!(
        state.autoreload_unwatched_warned.len(),
        1,
        "checking again must not warn about the same path twice"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **The shipped default loading scene never warns**: with nothing
/// configured, `pane::loading_source` resolves under the shipped `qml/`
/// directory, which [`Solium::warn_about_unwatched_configured_paths`] pads
/// the watched roots with for exactly this reason.
#[test]
fn does_not_warn_for_the_shipped_default_loading_scene() {
    if std::env::var_os("SOLIUM_LOADING").is_some() {
        return;
    }
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.autoreload_settings.automatic = true;

    state.warn_about_unwatched_configured_paths();

    assert!(
        state.autoreload_unwatched_warned.is_empty(),
        "the shipped default must not be reported as unwatched: {:?}",
        state.autoreload_unwatched_warned
    );
}

/// **`automatic = false` warns about nothing**, however configured: the
/// warning is about what automatic reload does not cover, so it has nothing
/// to say while automatic reload is off altogether.
#[test]
fn automatic_false_warns_about_nothing() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.autoreload_settings.automatic = false;
    state.loading.scene = Some("/does/not/exist/anywhere/Loading.qml".to_string());

    state.warn_about_unwatched_configured_paths();

    assert!(state.autoreload_unwatched_warned.is_empty());
}

/// **What the notice at the end of a reload is looking at.**
///
/// The recovery half of #116 shipped with no test at all, which is how the
/// inversion below went out with it. This is the decision itself: three
/// answers, and the third is the one that matters — a *lost* desktop and an
/// *empty* workspace are the same picture, so `None` and `Some(true)` are
/// different claims and the empty cases must not come back as "everything
/// is off stage".
#[test]
fn a_desktop_is_off_stage_only_when_there_is_a_desktop_and_it_is_off() {
    let screens = two_monitors().to_vec();
    // Each a window's slot and where it is drawn: both living on the first
    // monitor, one drawn there and one carried clear of every screen.
    let slot = at(100, 100, 800, 600);
    let on = (slot, slot.to_f64());
    let off = (slot, at(-4000, 0, 1920, 1080).to_f64());

    assert_eq!(
        nothing_on_stage([off], &screens),
        Some(true),
        "a window carried clear of every screen is the picture this warns about"
    );
    // Rules out `all(..)` for `any(..)` twice over: one window still on a
    // screen is a desktop that is there, whichever order they come in.
    assert_eq!(
        nothing_on_stage([off, on], &screens),
        Some(false),
        "one window still on a screen means the desktop is not lost"
    );
    assert_eq!(
        nothing_on_stage([on, off], &screens),
        Some(false),
        "and the answer does not depend on which window is looked at first"
    );

    // Neither of these is a question with an answer, and neither may come
    // back as `Some(true)`: the caller warns on exactly that, and a session
    // with nothing open would then be told its desktop had been carried
    // away.
    assert_eq!(
        nothing_on_stage([], &screens),
        None,
        "no windows is not an off-stage desktop"
    );
    assert_eq!(
        nothing_on_stage([on], &[]),
        None,
        "and neither is no screens, which is every window off every one of \
         them by arithmetic"
    );
}

/// **#134's third review: a window is on stage only on a monitor that
/// draws it.**
///
/// Two monitors side by side, and the left one's workspace 2 as the shipped
/// `workspaces.lua` parks it: the slot tiling gave it on the left monitor,
/// drawn 1.06 screens to the right -- squarely on the right monitor. The
/// renderer draws a pane only on the monitors its slot is on
/// (`render::drawn_on`), so none of it is on screen, and asked against
/// every screen, as this was, it was on stage.
#[test]
fn a_window_is_on_stage_only_on_a_monitor_that_draws_it() {
    let screens = two_monitors().to_vec();
    let slot = at(12, 12, 1896, 1056);
    let carried = Rectangle::<f64, Logical>::new(
        (12.0 + 1920.0 * 1.06, 12.0).into(),
        (1896.0, 1056.0).into(),
    );

    assert_eq!(
        nothing_on_stage([(slot, carried)], &screens),
        Some(true),
        "a window living on the left monitor, carried onto the right one, is on stage \
         -- where the right monitor never draws it"
    );
    // Rules out asking only the slot.
    assert_eq!(
        nothing_on_stage([(slot, slot.to_f64())], &screens),
        Some(false),
        "the same window drawn where it lives is on stage"
    );
    let right = at(1932, 12, 1896, 1056);
    assert_eq!(
        nothing_on_stage([(right, right.to_f64())], &screens),
        Some(false),
        "and so is one living and drawn on the right monitor"
    );
    // A slot across the bezel is on both, so a transform may carry it
    // around either: containment, not a single "its monitor".
    let straddling = at(1800, 100, 400, 600);
    let moved = Rectangle::<f64, Logical>::new((2000.0, 100.0).into(), (400.0, 600.0).into());
    assert_eq!(
        nothing_on_stage([(straddling, moved)], &screens),
        Some(false),
        "a window across the bezel moved onto the right monitor is still on stage"
    );
}

/// **#134's sixth review: a desk is put away when its selection is headed
/// anywhere, or to nothing.** See [`put_away`]. Until then it was put away
/// only when its whole screen was carried clear of itself, so a desk
/// parked by a work area narrower than the screen was not, and before the
/// fifth review a fade to nothing was not either.
#[test]
fn a_desk_is_put_away_when_its_selection_is_headed_anywhere_or_to_nothing() {
    use crate::group::Shift;
    let by = |dx: f64, dy: f64| Shift {
        dx,
        dy,
        ..Shift::NONE
    };

    assert!(
        put_away(by(1920.0 * 1.06, 0.0)),
        "a workspace parked a screen and a bit to the right, as the shipped `spread` parks \
         one, is on stage"
    );
    assert!(
        put_away(by(-1920.0, 0.0)) && put_away(by(0.0, 1080.0)),
        "a workspace exactly a screen to the left, or a screen down, is on stage"
    );
    assert!(
        put_away(by(-(1920.0 - 64.0), 0.0)) && put_away(by(0.0, 1080.0 - 64.0)),
        "a workspace parked a work area away at `spread = 1.0`, with a 64-pixel panel across \
         the slide, is on stage"
    );
    assert!(
        put_away(by(100.0, 40.0)),
        "a desk nudged and left on its screen is on stage"
    );
    assert!(
        put_away(Shift {
            opacity: 0.0,
            ..Shift::NONE
        }),
        "a desk faded to nothing where it is is on stage"
    );

    assert!(!put_away(Shift::NONE), "the desk in view is put away");
    assert!(
        !put_away(Shift {
            opacity: 0.4,
            ..Shift::NONE
        }),
        "a dimmed desk is put away"
    );
    assert!(
        !put_away(Shift {
            matrix: crate::mat4::Mat4::rotate_z(0.3),
            ..Shift::NONE
        }),
        "a turned desk is put away"
    );
}

/// **The hit tests' half of the same rule**: a point is on a window only
/// on a monitor that draws it. See [`shown_at`].
#[test]
fn a_point_is_on_a_window_only_on_a_monitor_that_draws_it() {
    let screens = two_monitors();
    let left = at(12, 12, 1896, 1056);
    let on_left = Point::<f64, Logical>::from((500.0, 500.0));
    let on_right = Point::<f64, Logical>::from((2500.0, 500.0));

    assert!(
        !shown_at(left, false, on_right, &screens),
        "a window living on the left monitor is under a point on the right one -- \
         which is where its hidden workspace is carried, and never drawn"
    );
    assert!(
        shown_at(left, false, on_left, &screens),
        "and it is under one on its own"
    );
    assert!(
        shown_at(at(1800, 100, 400, 600), false, on_right, &screens),
        "a window across the bezel is drawn on both monitors, so on either"
    );
    assert!(
        !shown_at(left, false, (-10.0, 500.0).into(), &screens),
        "a point on no monitor is on nothing, because nothing is drawn there"
    );
    assert!(
        shown_at(left, false, on_right, &[]),
        "with no monitors at all nothing is known, and nothing is refused"
    );
    // #126: what is left of a window whose client has gone, on the monitor
    // that draws it, and with no monitors to go by.
    assert!(
        !shown_at(left, true, on_left, &screens) && !shown_at(left, true, on_right, &[]),
        "a point is on what is left of a window whose client has gone"
    );
}

/// **The focus half measures a frame by its rectangle; the renderer by its
/// bleed, and not at all when it is transformed.** Pinned as they stand, so
/// that the two are not taken for one rule -- the note beside
/// [`crate::render::drawn_on`] says why it is harmless and why it matters.
///
/// A window living on the left monitor and drawn just past its left edge,
/// tilted as a mode might tilt it: [`staged`] says it is off stage, from
/// the rectangle, while `render::elements` skips its bleed cull for any
/// frame with a matrix or a deform and so still draws it. The renderer
/// needs a GPU the build container does not have, so its half is pinned by
/// the source text, as `render.rs`'s drag-icon test pins its ordering --
/// by its tokens, with the whitespace squeezed out of both, so a rustfmt
/// reflow of that line moves nothing here and a change to the condition
/// does.
#[test]
fn the_focus_half_counts_a_frame_by_its_rectangle_where_the_renderer_counts_its_bleed_and_every_transform()
 {
    let screens = two_monitors();
    let slot = at(12, 12, 400, 300);
    let tilted = Frame {
        rect: Rectangle::<f64, Logical>::new((-500.0, 12.0).into(), (400.0, 300.0).into()),
        matrix: crate::mat4::Mat4::rotate_z(0.3),
        ..Frame::real(slot)
    };
    assert!(
        !staged(slot, tilted, &screens),
        "a window whose rectangle is off every screen counts as on stage: the focus half \
         is measuring something other than its rectangle now, and the note beside \
         `render::drawn_on` is out of date"
    );

    let tokens = |text: &str| text.split_whitespace().collect::<String>();
    assert!(
        tokens(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/render.rs"
        )))
        .contains(&tokens(
            "if frame.matrix.is_identity()
                     && frame.deform.is_none()
                     && !reaches(bled, prepared.slots.reach(pane), screen)"
        )),
        "`render::elements` no longer culls by the bleed, or now culls a transformed frame: \
         the note beside `render::drawn_on` is out of date"
    );
}

/// **And when it looks, which is the whole of the fault.**
///
/// `everything_is_off_stage` ran at `self.clock.now()`, immediately after
/// the reload's three dispatches. A workspace slide is an animation, and at
/// its own start instant it has moved nothing — so the notice described the
/// session that had just been thrown away. A reload that *rescued* an
/// off-stage desktop warned about it, and one that carried the desktop off
/// said nothing at all.
///
/// Asked of a real [`Groups`] carrying a real [`present::Transform`],
/// through the same `Shift::apply` that [`Solium::drawn_at`] ends in. Both
/// instants are asserted, because "sampled later" is only worth anything if
/// the earlier sample really does give the other answer — otherwise this
/// would pass against the code it is pinned against.
///
/// **The honest limit.** This pins [`SETTLED`] and what it is for; it
/// cannot see `everything_is_off_stage` choosing to sample somewhere else,
/// because that method needs a `Display` and a mapped `Space` and so cannot
/// be called here at all. Setting `SETTLED` back to `ZERO` — which is what
/// the code did — fails the second assertion.
#[test]
fn a_slide_that_has_not_started_yet_is_not_where_the_desks_end_up() {
    let start = Duration::from_secs(10);
    let screens = two_monitors().to_vec();
    let real = at(100, 100, 800, 600);

    let mut groups = crate::group::Groups::default();
    groups.declare(
        "desk-2",
        crate::group::Selection {
            members: vec![crate::group::Member::Window(7)],
            on: None,
        },
        start,
    );
    // A screen and a bit to the left, over 300ms: `workspaces.lua`'s own
    // numbers, and what a reload onto another workspace asks for.
    groups.present(
        "desk-2",
        crate::group::Shift {
            dx: -1920.0 * 1.06,
            ..crate::group::Shift::NONE
        },
        start,
        Duration::from_millis(300),
        present::Curve::OutCubic,
    );

    let where_it_is = |now| {
        (
            real,
            groups.on_window(7, None, now).apply(Frame::real(real)).rect,
        )
    };

    assert_eq!(
        nothing_on_stage([where_it_is(start)], &screens),
        Some(false),
        "at the instant the reload finishes, the slide it started has moved \
         nothing -- so this is the session before the reload, and reporting it \
         is reporting the wrong one"
    );
    assert_eq!(
        nothing_on_stage([where_it_is(start + SETTLED)], &screens),
        Some(true),
        "SETTLED is not far enough ahead for the transforms this reload started \
         to have landed, so the notice still describes the previous session"
    );
}

/// **#115: a client's limits, in the terms a layout reads them.**
mod limits {
    use super::*;

    /// **A limit is read in the window's own terms, frame included**: the
    /// frame's insets added to each side the client limited, and nothing to a
    /// side it did not, so 0 still reads as no limit. A titlebar is on top.
    #[test]
    fn a_limit_is_read_in_the_windows_own_terms_frame_included() {
        let titled = Insets {
            top: 32,
            left: 2,
            right: 2,
            bottom: 2,
        };
        let in_pane = super::snapshot::in_pane;
        assert_eq!(
            in_pane(Size::from((800, 600)), titled),
            Some(Size::from((804, 634)))
        );
        assert_eq!(
            in_pane(Size::from((800, 0)), titled),
            Some(Size::from((804, 0)))
        );
        assert_eq!(in_pane(Size::from((0, 0)), titled), None);
    }

    /// **X11 hints that say nothing are no limit**: smithay answers `None`
    /// for a flag the client left unset, and that side is 0 both ways.
    #[test]
    fn x11_hints_that_say_nothing_are_no_limit() {
        assert_eq!(
            Limits::from_hints(None, Some(Size::from((1024, 768)))),
            Limits {
                min: Size::from((0, 0)),
                max: Size::from((1024, 768)),
            }
        );
        assert_eq!(
            Limits::from_hints(Some(Size::from((320, 200))), None),
            Limits {
                min: Size::from((320, 200)),
                max: Size::from((0, 0)),
            }
        );
    }

    /// **A limit past any screen is read as the most there is.** A client
    /// may say `i32::MAX`, and a column laid out that wide put the next one
    /// past the end of `i32`.
    #[test]
    fn a_limit_past_any_screen_is_read_as_the_most_there_is() {
        assert_eq!(
            Limits::from_hints(
                Some(Size::from((i32::MAX, 0))),
                Some(Size::from((i32::MAX, i32::MAX)))
            ),
            Limits {
                min: Size::from((snapshot::MOST, 0)),
                max: Size::from((snapshot::MOST, snapshot::MOST)),
            }
        );
    }
}

#[test]
fn a_frame_reserves_what_it_always_reserved() {
    // The three answers `insets_of` used to assemble from two tables,
    // now read off one value.
    assert_eq!(
        insets_for(&crate::pane::Frame::Pending),
        Insets {
            top: TITLEBAR_HEIGHT,
            ..Insets::NONE
        },
        "a frame that has not been built yet still reserves room for one, \
         or the window changes shape the moment it arrives"
    );
    assert_eq!(
        insets_for(&crate::pane::Frame::None),
        Insets::NONE,
        "a pane that will never have a frame reserves nothing -- a \
         titlebar's worth of blank space with no titlebar in it is what \
         an Electron application looked like here"
    );
    // The third answer -- that a built frame reserves what its decoration
    // asked for, on every side, so that a bar along the left and a border
    // are the same mechanism -- was asserted here against a `Styled` arm
    // carrying a plain `Insets`. That arm carries the `Decoration` itself
    // now, and `Decoration::new` is private to `decoration.rs`, so the case
    // cannot be written here.
    //
    // It *can* be written there, and is: `decoration.rs`'s tests build real
    // frames. The reason this file does not reach over and do the same is
    // not that a frame needs a GPU and a display -- it does not, and this
    // comment said so for a while. It is that `solium_qml_start` assigns
    // the one `QGuiApplication` without a lock, so the tests that bring Qt
    // up share a mutex, and that mutex is in the module where they live.
    // A second, unsynchronised starter in another module is a data race.
}

/// Drawing is unclipped; input is not. A spike reaching over the next
/// window must not eat that window's clicks — the failure mode is a
/// neighbour that has silently stopped responding, with nothing on screen
/// to explain it.
///
/// **It passed the moment it was written, and that is the point.** Every
/// hit-test in this file — `chrome_under`, `decorated_under`,
/// `window_under` and `surface_under` — reaches its pane through
/// [`Solium::pane_outer`], and the two that own a decoration gate on
/// `drawn.rect.contains(location)` before a layer is asked anything at all.
/// So the invariant holds by construction and nothing *states* it: the
/// canvas is a rectangle that exists, is larger, is right there in
/// `decoration.rs`, and is exactly what someone fixing "my glow does not
/// take clicks" would reach for.
///
/// **What it is and is not.** It is the two rectangles' relationship, in
/// one place, with the reason written down; it is not a guard on the
/// hit-tests, and it adds no machine-checked coverage that `decoration.rs`
/// did not already have. Both controls below were run, and between them
/// they are the honest limit of this test: the one it fails is pinned
/// twice over elsewhere, and the regression it is named for is pinned
/// nowhere.
///
/// | control | measured |
/// |---|---|
/// | `canvas` returning `outer` — the state before bleed existed | fails on the first assertion, but so do `decoration::tests::a_canvas_is_the_pane_grown_by_its_bleed` and `no_bleed_means_the_canvas_is_the_pane`, which pin it already |
/// | the frame band **and** `decorated_under` switched to the canvas, through `decoration::spread` | the whole suite still passes, 201 of 201 |
///
/// The regression is instead caught with a real pointer, by hand: two panes
/// side by side under the `bleedy` fixture, a press
/// 60px into the first one's bleed and 20px inside the second, and the
/// second takes focus. Run against the canvas-hit-test build above, the
/// *first* window takes it and the second is left unfocused — a neighbour
/// that has silently stopped responding, exactly as described.
///
/// A point 30px to the *left* of the pane: inside a canvas that bled 50, and
/// outside the pane on the only axis that matters.
#[test]
fn a_point_in_the_bleed_is_not_in_the_pane() {
    let outer = Rectangle::<i32, Logical>::new((100, 100).into(), (200, 200).into());
    let bleed = crate::style::Bleed {
        top: 50,
        right: 50,
        bottom: 50,
        left: 50,
    };
    let canvas = crate::decoration::canvas(outer, bleed);
    let in_bleed = smithay::utils::Point::<f64, smithay::utils::Logical>::from((70.0, 120.0));
    assert!(canvas.to_f64().contains(in_bleed));
    assert!(!outer.to_f64().contains(in_bleed));
}

/// An ordinary framed window: 400x300 at (100, 100) under a titlebar and
/// nothing else reserved, which is the shape both of #108's symptoms were
/// reported on.
fn framed() -> (Rectangle<i32, Logical>, Insets) {
    (
        Rectangle::new((100, 100).into(), (400, 300).into()),
        Insets {
            top: TITLEBAR_HEIGHT,
            ..Insets::NONE
        },
    )
}

/// What [`Solium::pane_chrome`] makes of a point of that window, with the
/// pane drawn where the layout put it — no transform, so a screen point
/// and a pane-local point differ only by the pane's corner.
fn chrome_at(point: (f64, f64)) -> Option<Chrome> {
    let (outer, insets) = framed();
    let location = Point::<f64, Logical>::from(point);
    chrome_of(
        on_frame(outer.size, insets, location - outer.loc.to_f64()),
        resize::border_edges(outer, location),
    )
}

/// The whole of [`claim_of`] at a point of that window, with the two links
/// above the chrome supplied by the caller: `surface` is an interactive
/// scripted surface above the windows claiming the point, `mode` is a
/// script grab held — `sol.grab(true)`, which is overview.
fn claim_at(point: (f64, f64), surface: bool, mode: bool) -> Claim {
    claim_of(surface, mode, chrome_at(point))
}

/// **Issue #108, and the assertion the fix is actually for.**
///
/// The cursor and the press must be reading the same answer, because the
/// bug was that they were not. The frame's band and the resize border
/// genuinely overlap — the top `RESIZE_BORDER` pixels of a titlebar are
/// within reach of the window's top edge — and the press had always
/// resolved that in the frame's favour while the pointer resolved it not
/// at all, leaving whatever a CSD client had set for its own shadow's
/// resize affordance. So in that band the pointer drew a resize arrow and
/// a drag moved the window.
///
/// What is pinned here is that no point can be claimed by both: whatever
/// [`claim_of`] answers is *the* answer, and the cursor is a function of it
/// rather than of a second hit test. A test that checked only the
/// edge-to-icon mapping would pass with the second symptom still in place,
/// which is why the sweep below is the body of this test and the named
/// points are only the landmarks.
///
/// **The chrome is one link of three, and the first version of this test
/// swept only that one.** It was green while the pointer still promised a
/// resize over an overview thumbnail and over the bottom edge of a scripted
/// bar, because it never held a script grab and never put a surface over
/// the point — so it exercised exactly the world in which the bug does not
/// appear. The sweep is now run in all four worlds, and the deferring ones
/// are checked against the count of pixels the chrome *would* have claimed,
/// so a chain that quietly stopped deferring could not leave this passing.
#[test]
fn the_pointer_and_the_press_cannot_claim_different_things() {
    let (outer, insets) = framed();

    // The overlap is real, not theoretical -- without this the sweep's
    // "never both" would be vacuously true and would stay true if somebody
    // shrank the titlebar to nothing.
    let contested = Point::<f64, Logical>::from((300.0, 104.0));
    assert!(
        on_frame(outer.size, insets, contested - outer.loc.to_f64()),
        "four pixels below the top edge is inside a {TITLEBAR_HEIGHT}px \
         titlebar"
    );
    assert_ne!(
        resize::border_edges(outer, contested),
        ResizeEdge::None,
        "and inside the top resize border, which is what the two disagreed \
         about"
    );
    // The frame takes it, because the frame is what a press there does.
    assert_eq!(chrome_at((300.0, 104.0)), Some(Chrome::Frame));
    assert_eq!(
        claim_at((300.0, 104.0), false, false).cursor(),
        Some(CursorIcon::Default),
        "the band that moves the window must not draw a resize cursor: \
         that is #108's second symptom"
    );

    // Four pixels *above* the top edge is outside the window, so the frame
    // has no claim on it and the border does. This is where dragging the
    // top edge still works, and it now says so.
    assert_eq!(
        chrome_at((300.0, 96.0)),
        Some(Chrome::Resize(ResizeEdge::Top))
    );

    // The first symptom: the bottom-right corner resizes, and now looks
    // like it. Nothing is reserved along the bottom or the right, so the
    // corner is the client's pixels and the border's claim alone.
    let corner = (497.0, 397.0);
    assert_eq!(
        chrome_at(corner),
        Some(Chrome::Resize(ResizeEdge::BottomRight))
    );
    assert_eq!(
        claim_at(corner, false, false).cursor(),
        Some(CursorIcon::NwseResize)
    );

    // The middle of the client is nobody's chrome, which is what leaves a
    // client free to name its own cursor over its own window.
    assert_eq!(chrome_at((300.0, 250.0)), None);
    assert_eq!(chrome_at((900.0, 900.0)), None);

    // **A mode holds the grab: overview.** `lua/overview.lua` sets it and
    // hit-tests its own thumbnails, and `pointer_button` hands every press
    // to the mode before it looks at any chrome -- so a press on that same
    // corner focuses the window and leaves overview. Offering
    // `NwseResize` there is the pointer describing an action that will not
    // happen, which is the whole of #108, on `super+space`.
    assert_eq!(claim_at(corner, false, true), Claim::Mode);
    assert_eq!(
        claim_at(corner, false, true).cursor(),
        None,
        "in a mode the press is the mode's, so the pointer promises nothing"
    );
    assert_eq!(claim_at((300.0, 104.0), false, true).cursor(), None);

    // **A scripted surface takes the press.** The bottom eight pixels of a
    // `layer = "top"` bar are within reach of a maximised window's top
    // edge, and the tweaks panel is a full-height overlay down the right of
    // one. The press goes to the panel; the border cursor would have said
    // it resized the window.
    assert_eq!(claim_at(corner, true, false), Claim::Surface);
    assert_eq!(
        claim_at(corner, true, false).cursor(),
        None,
        "the press is the surface's, so the pointer leaves the shape to it"
    );
    assert_eq!(claim_at((300.0, 104.0), true, false).cursor(), None);

    // And the order between the two, which is the order `pointer_button`
    // asks them in: a surface above the windows is offered the press before
    // the mode is consulted.
    assert_eq!(claim_of(true, true, None), Claim::Surface);
    assert_eq!(claim_of(false, false, None), Claim::Nothing);
    assert_eq!(claim_of(false, false, None).cursor(), None);

    // And the whole neighbourhood of the window, a pixel at a time, in
    // each of the four worlds the two links above the chrome make.
    let mut chrome_pixels = 0_u32;
    let mut deferred_pixels = 0_u32;
    for (surface, mode) in [(false, false), (true, false), (false, true), (true, true)] {
        for y in 80..=420 {
            for x in 80..=520 {
                let location = Point::<f64, Logical>::from((f64::from(x), f64::from(y)));
                let framed_here = on_frame(outer.size, insets, location - outer.loc.to_f64());
                let edges = resize::border_edges(outer, location);
                let chrome = chrome_of(framed_here, edges);
                if !surface && !mode && chrome.is_some() {
                    chrome_pixels += 1;
                }
                if (surface || mode) && chrome.is_some() {
                    deferred_pixels += 1;
                }
                match claim_of(surface, mode, chrome) {
                    Claim::Surface => {
                        assert!(
                            surface,
                            "nothing was over {location:?} and the pointer \
                             stood aside for it"
                        );
                        assert_eq!(claim_of(surface, mode, chrome).cursor(), None);
                    }
                    Claim::Mode => {
                        assert!(
                            mode && !surface,
                            "no mode holds the grab at {location:?}, or a \
                             surface should have taken it first"
                        );
                        assert_eq!(claim_of(surface, mode, chrome).cursor(), None);
                    }
                    Claim::Chrome(Chrome::Frame) => {
                        assert!(
                            !surface && !mode,
                            "a press at {location:?} would never reach the \
                             frame, and the pointer says it would"
                        );
                        assert!(
                            framed_here,
                            "a press at {location:?} would not hit the \
                             frame, but the pointer says it would"
                        );
                    }
                    Claim::Chrome(Chrome::Resize(edges)) => {
                        assert!(
                            !surface && !mode,
                            "a press at {location:?} would never reach the \
                             resize border, and the pointer offered to drag \
                             it {edges:?}"
                        );
                        assert!(
                            !framed_here,
                            "a press at {location:?} moves the window, and \
                             the pointer offered to resize it {edges:?}"
                        );
                        assert_ne!(edges, ResizeEdge::None);
                        assert_eq!(
                            Chrome::Resize(edges).cursor(),
                            resize::cursor(edges),
                            "the cursor over a border is the border's own, \
                             whatever else is on screen"
                        );
                    }
                    Claim::Nothing => assert!(
                        !surface && !mode && !framed_here && edges == ResizeEdge::None,
                        "the compositor asserts nothing at {location:?} \
                         while claiming to own it"
                    ),
                }
            }
        }
    }

    // The deferring worlds are only worth sweeping if the chrome had
    // something to say at those pixels, which is what made the first
    // version of this test green over a live disagreement. Three of the
    // four worlds defer, so the same pixels are counted three times.
    assert!(chrome_pixels > 0, "the sweep never crossed any chrome");
    assert_eq!(
        deferred_pixels,
        chrome_pixels * 3,
        "every pixel the chrome would have claimed must be one the pointer \
         gave up in each of the three worlds where the press never gets \
         there"
    );
}

/// **A scene's shape is asserted only where it takes the press** (#373):
/// where the claim is the scene's, the shape it asks for, over a mode and a
/// frame below it alike, and nothing for its arrow; where the press is a
/// mode's, a frame's or nobody's, the claim's own answer, whatever a scene
/// asked.
#[test]
fn a_scenes_shape_is_asserted_only_where_it_takes_the_press() {
    let asked = Some(CursorIcon::Crosshair);
    assert_eq!(claim_of(true, false, None).cursor_over(asked), asked);
    assert_eq!(
        claim_of(true, true, Some(Chrome::Frame)).cursor_over(asked),
        asked,
        "a scene over a mode and a frame"
    );
    assert_eq!(
        claim_of(true, false, None).cursor_over(None),
        None,
        "the arrow asks for nothing"
    );
    assert_eq!(claim_of(false, true, None).cursor_over(asked), None);
    assert_eq!(
        claim_of(false, false, Some(Chrome::Frame)).cursor_over(asked),
        Some(CursorIcon::Default)
    );
    assert_eq!(claim_of(false, false, None).cursor_over(asked), None);
}

/// The frame band is the insets and only the insets.
///
/// Two directions, because getting it wrong either way is a bug with a
/// face: too wide and a strip of the client stops taking clicks, too
/// narrow and the titlebar has a dead line along one edge. The outer bound
/// is asserted separately -- it is what stops every point above a window
/// from counting as its titlebar, which is the mistake a "not the client
/// rect" test makes on its own.
#[test]
fn the_frame_band_is_what_the_insets_reserved() {
    let (outer, insets) = framed();
    let local = |x: f64, y: f64| Point::<f64, Logical>::from((x, y));

    assert!(on_frame(outer.size, insets, local(200.0, 0.0)));
    assert!(on_frame(
        outer.size,
        insets,
        local(200.0, f64::from(TITLEBAR_HEIGHT) - 1.0)
    ));
    assert!(
        !on_frame(outer.size, insets, local(200.0, f64::from(TITLEBAR_HEIGHT))),
        "the first row below the bar is the client's"
    );
    assert!(
        !on_frame(outer.size, insets, local(200.0, -1.0)),
        "a point above the window is not its titlebar"
    );
    assert!(
        !on_frame(outer.size, insets, local(200.0, 500.0)),
        "nor is a point below it, which reserves nothing"
    );
    assert!(
        !on_frame(outer.size, Insets::NONE, local(200.0, 0.0)),
        "a decoration that reserves nothing owns no band, and its clicks \
         belong to the window under it"
    );
}

/// One pane of a stack, as the pure rules see it: where it is drawn, and
/// what its frame reserves.
///
/// No presentation transform, so the drawn rect *is* the outer rect and a
/// screen point differs from a pane-local one only by the pane's corner.
/// That is the situation #111 was reported in — two ordinary overlapping
/// windows on the desktop, neither of them in a mode — and it keeps the
/// arithmetic below readable enough to check by hand. A built decoration is
/// assumed, which both windows in the report had.
#[derive(Clone, Copy)]
struct Stacked {
    outer: Rectangle<i32, Logical>,
    insets: Insets,
    /// Whether the client has arrived. `false` is a pane still loading: it
    /// is drawn and it covers, its frame's buttons work, and it has no
    /// window for a resize border to drag.
    window: bool,
    /// Whether the layout owns this pane. `false` is an X11 menu, tooltip
    /// or dropdown the client placed itself.
    managed: bool,
    /// Whether this pane paints anything. `false` is a pane held at opacity
    /// zero — which on this desktop means one between its close animation
    /// landing and its client acting on the request. It has a rectangle and
    /// it covers nothing.
    shows: bool,
}

impl Stacked {
    /// A framed window at a corner: a titlebar across the top and nothing
    /// else reserved, which is [`framed`]'s shape at an arbitrary place.
    fn window(at: (i32, i32), size: (i32, i32)) -> Self {
        Self {
            outer: Rectangle::new(at.into(), size.into()),
            insets: Insets {
                top: TITLEBAR_HEIGHT,
                ..Insets::NONE
            },
            window: true,
            managed: true,
            shows: true,
        }
    }

    /// The same pane with its application not yet arrived.
    const fn loading(mut self) -> Self {
        self.window = false;
        self
    }

    /// The same pane placed by its own client: a menu, a tooltip, a
    /// dropdown. It draws, so it covers; it is nothing's to resize or move.
    const fn unmanaged(mut self) -> Self {
        self.managed = false;
        self
    }

    /// The same pane held at opacity zero: asked to close and waiting on a
    /// client that has not answered. It is still in the stack, still the
    /// topmost thing at this rectangle, and on screen it is not there.
    const fn invisible(mut self) -> Self {
        self.shows = false;
        self
    }

    /// What [`Solium::pane_chrome`] makes of a point, out of the same two
    /// functions in the same order it uses them.
    ///
    /// [`chrome_offered`] and [`pane_hit_of`] are called here rather than
    /// reimplemented, which is the difference between a fixture and a
    /// second copy of the rule: `covers` is what `pane_chrome` passes —
    /// the *drawn* rect containing the point, which with no presentation
    /// transform is this rect — and both of `chrome_offered`'s gates apply
    /// exactly as they do there. A hand-rolled composition here is how the
    /// covers-before-chrome mistake could come back with every test still
    /// green.
    fn hit(self, point: (f64, f64)) -> PaneHit<Chrome> {
        let location = Point::<f64, Logical>::from(point);
        // Both halves of `Frame::covers`, in the same order and for the
        // same reason `pane_chrome` asks them: the rectangle says where the
        // pane would be drawn, and `shows` says whether it is drawn at all.
        let covers = self.shows && self.outer.to_f64().contains(location);
        let framed = covers
            && on_frame(
                self.outer.size,
                self.insets,
                location - self.outer.loc.to_f64(),
            );
        pane_hit_of(
            chrome_offered(
                self.shows,
                self.managed,
                self.window,
                framed,
                resize::border_edges(self.outer, location),
            ),
            covers,
        )
    }
}

/// The walk as it stood before #111: every pane's frame over the whole
/// stack, and only then every pane's resize border, with a pane that merely
/// *covers* the point stopping nothing.
///
/// Kept rather than deleted so the bug is pinned and not only the fix. A
/// test that asserts the new answer alone stays green against a walk that
/// never occludes anything — which is how this fault survived #108's
/// rewrite of the very same function, and why each test below measures both
/// rules at the same point.
///
/// `stack` is topmost-first, as [`Solium::chrome_under`]'s `rev` makes it.
fn two_pass(stack: &[Stacked], point: (f64, f64)) -> Option<Chrome> {
    let claimed = |frames: bool| {
        stack.iter().find_map(|pane| match pane.hit(point) {
            // Drawn or not made no difference to this walk: it asked each
            // pane for chrome and took the first that answered.
            PaneHit::Chrome(Chrome::Frame) | PaneHit::Halo(Chrome::Frame) if frames => {
                Some(Chrome::Frame)
            }
            PaneHit::Chrome(Chrome::Resize(edges)) | PaneHit::Halo(Chrome::Resize(edges))
                if !frames =>
            {
                Some(Chrome::Resize(edges))
            }
            _ => None,
        })
    };
    claimed(true).or_else(|| claimed(false))
}

/// The walk as `ab11731` first fixed #111: one pass, topmost first, with
/// *any* chrome claim winning outright whether or not the pane claiming it
/// draws anything at that point.
///
/// The second control, kept for the same reason [`two_pass`] is. It got the
/// covered titlebar right — that was the fix — and it got a halo over a
/// lower pane's drawn chrome wrong, because "which window is on top" was
/// asked at a pixel the upper window does not occupy. Every case below
/// measures all three rules at the same point, so what each one gets right
/// and wrong is written down rather than remembered.
fn halo_wins(stack: &[Stacked], point: (f64, f64)) -> Option<Chrome> {
    for pane in stack {
        match pane.hit(point) {
            PaneHit::Chrome(chrome) | PaneHit::Halo(chrome) => return Some(chrome),
            PaneHit::Client => return None,
            PaneHit::Miss => {}
        }
    }
    None
}

/// **Issue #111, as reported: a titlebar took clicks through the window
/// covering it.**
///
/// Two overlapping windows. A press on the *top* one, at a point where the
/// lower one's titlebar happened to lie underneath, focused and raised the
/// lower window. The walk searched only for chrome, so the top window —
/// whose client covers the point and which therefore had no chrome to
/// offer — did not stop the descent, and the lower window's titlebar was
/// found beneath it.
///
/// The last assertion is the control, and it was run red before the fix:
/// with the first assertion pointed at `two_pass` the test fails with
/// `Some(Frame)` against an expected `None`, which is the reported
/// behaviour reproduced in a unit test rather than on hardware.
#[test]
fn a_covered_titlebar_does_not_take_the_click() {
    let lower = Stacked::window((100, 100), (400, 300));
    let upper = Stacked::window((60, 60), (400, 300));
    let stack = [upper, lower];

    // (300, 116) is 56px down into the upper window: past its 32px titlebar
    // and 160px clear of its nearest resize border, so it is that window's
    // client and nothing else. The same point is 16px down into the lower
    // window, inside its titlebar and clear of its top border -- so the two
    // windows genuinely disagree here, which is what makes the walk's
    // answer worth anything.
    let point = (300.0, 116.0);
    assert_eq!(
        upper.hit(point),
        PaneHit::Client,
        "the top window covers this point with its client"
    );
    assert_eq!(
        lower.hit(point),
        PaneHit::Chrome(Chrome::Frame),
        "and the lower window's titlebar is underneath it, which is the \
         whole setup"
    );

    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(point))),
        None,
        "a press on the top window's client is the client's, and the \
         titlebar buried under it may not have it"
    );
    assert_eq!(
        two_pass(&stack, point),
        Some(Chrome::Frame),
        "the walk this replaced hands the point to the covered titlebar, \
         which is the bug"
    );
    assert_eq!(
        halo_wins(&stack, point),
        None,
        "and the one-pass rule that replaced it got this case right -- \
         what it got wrong is the halo, below"
    );
}

/// **A resize border hanging outside its own window loses to anything a
/// lower pane actually draws, and beats bare desktop.**
///
/// A border reaches [`resize::RESIZE_BORDER`] pixels *outside* the window it
/// belongs to, over whatever is behind. Out there the pane draws nothing, so
/// the claim is not backed by a single pixel and "which window is on top"
/// has no answer: an upper window's bottom edge floating four pixels above a
/// lower window's close button is not on top of that button in any sense a
/// user would recognise. It is beside it. The button is what is drawn there
/// and the button takes the press.
///
/// The halo still wins over the desktop, which is the only reason an edge
/// can be grabbed from outside at all — and its *inside* half still wins
/// outright, since there the pane does draw the pixel it is claiming.
///
/// Three rules are measured at every point: the corrected one, the
/// [`two_pass`] walk from before #111, and [`halo_wins`] as #111 was first
/// fixed. The first case is the one that separates them.
#[test]
fn a_halo_loses_to_what_a_lower_pane_draws() {
    let lower = Stacked::window((100, 100), (400, 300));
    // Overlapping the top-right of the lower window, where its close button
    // is. The bottom edge, y = 120, floats inside the lower window's
    // titlebar band -- drawn, and 30px clear of the lower window's own
    // right border, so the only things meeting here are one window's empty
    // margin and another window's buttons.
    let upper = Stacked::window((300, 20), (200, 100));
    let stack = [upper, lower];

    let button = (470.0, 124.0);
    assert_eq!(
        upper.hit(button),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
        "4px below the upper window's bottom edge is its resize border, and \
         outside everything that window draws"
    );
    assert_eq!(
        lower.hit(button),
        PaneHit::Chrome(Chrome::Frame),
        "and 24px down into the lower window's titlebar, which is drawn"
    );
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(button))),
        Some(Chrome::Frame),
        "the titlebar is drawn there and the border is not, so the press is \
         the button's"
    );
    assert_eq!(
        two_pass(&stack, button),
        Some(Chrome::Frame),
        "which the walk from before #111 also answered -- it was not wrong \
         here, only wrong about why, having never asked which pane was on \
         top at all"
    );
    assert_eq!(
        halo_wins(&stack, button),
        Some(Chrome::Resize(ResizeEdge::Bottom)),
        "where #111's first fix drew NsResize over a visible close button \
         and started a resize grab on it"
    );

    // The same halo over a lower window's *client*, which is drawn just as
    // surely as its titlebar is. Nothing is offered and the client keeps
    // its own cursor: a window's margin does not reach through the window
    // under it.
    let deeper = Stacked::window((300, 20), (200, 180));
    let body = (470.0, 204.0);
    assert_eq!(
        deeper.hit(body),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom))
    );
    assert_eq!(lower.hit(body), PaneHit::Client);
    assert_eq!(
        topmost_chrome([deeper, lower].iter().map(|pane| pane.hit(body))),
        None
    );
    assert_eq!(
        two_pass(&[deeper, lower], body),
        Some(Chrome::Resize(ResizeEdge::Bottom)),
        "and here the older walk is wrong too, so neither rule this \
         replaces got the halo right"
    );

    // Over the desktop the halo is the best claim there is, and it must
    // still win: this is what makes an edge grabbable from outside, and a
    // rule that demanded a pane draw what it claims would take every
    // window's outer border away.
    let sky = (400.0, 16.0);
    assert_eq!(
        upper.hit(sky),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Top)),
        "4px above the upper window's top edge"
    );
    assert_eq!(lower.hit(sky), PaneHit::Miss);
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(sky))),
        Some(Chrome::Resize(ResizeEdge::Top)),
        "nothing is drawn under the halo, so the halo has it"
    );
    assert_eq!(two_pass(&stack, sky), Some(Chrome::Resize(ResizeEdge::Top)));
    assert_eq!(
        halo_wins(&stack, sky),
        Some(Chrome::Resize(ResizeEdge::Top))
    );

    // And the half of that border that lies *inside* its own window is
    // chrome outright: the pane draws the pixel it is claiming, so it wins
    // over everything below and is never downgraded to a halo. A rule that
    // answered `Client` wherever the drawn rect contained the point would
    // have swallowed it and left every window resizable only from outside.
    let inside = (300.0, 355.0);
    let alone = Stacked::window((60, 60), (400, 300));
    assert!(
        alone
            .outer
            .to_f64()
            .contains(Point::<f64, Logical>::from(inside))
    );
    assert_eq!(
        alone.hit(inside),
        PaneHit::Chrome(Chrome::Resize(ResizeEdge::Bottom))
    );
    assert_eq!(
        topmost_chrome([alone, lower].iter().map(|pane| pane.hit(inside))),
        Some(Chrome::Resize(ResizeEdge::Bottom)),
        "and it beats the lower window it is drawn over, which is the half \
         of #111's fix that was right"
    );
}

/// **A halo over two panes takes the topmost one's, and a lower pane's
/// border does not reach up through a window covering it.**
///
/// The tie-break the corrected rule needs and the covered case it must not
/// lose. Remembering a halo and carrying on is only safe if the *first* one
/// is kept: two stacked windows whose edges both hang over the same strip
/// of desktop are an ordinary sight, and the answer there is still the one
/// on top.
#[test]
fn the_topmost_halo_is_the_one_kept() {
    // Two windows 6px apart with a strip of desktop between them: the
    // upper's bottom border and the lower's top border both hang over it,
    // and they name opposite edges, so which one the walk keeps is
    // legible in the answer.
    let upper = Stacked::window((100, 100), (200, 100));
    let lower = Stacked::window((100, 206), (200, 100));
    let below = (200.0, 202.0);

    assert_eq!(
        upper.hit(below),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
        "2px below the upper window"
    );
    assert_eq!(
        lower.hit(below),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Top)),
        "and 4px above the lower one"
    );
    assert_eq!(
        topmost_chrome([upper, lower].iter().map(|pane| pane.hit(below))),
        Some(Chrome::Resize(ResizeEdge::Bottom)),
        "two halos over the same desktop, and the topmost is the one kept"
    );
    assert_eq!(
        topmost_chrome([lower, upper].iter().map(|pane| pane.hit(below))),
        Some(Chrome::Resize(ResizeEdge::Top)),
        "stack them the other way and the answer follows the stacking order"
    );

    // A lower window's border, under a window that covers where it hangs.
    // The border is the lower window's own and the point is still the upper
    // window's client: a halo is a claim on the desktop, not a tunnel.
    let over = Stacked::window((60, 150), (400, 200));
    assert_eq!(over.hit(below), PaneHit::Client);
    assert_eq!(
        topmost_chrome([over, upper].iter().map(|pane| pane.hit(below))),
        None,
        "the covering window's client owns it, and the border reaching up \
         from underneath does not"
    );
}

/// **An unmanaged pane occludes and offers nothing.**
///
/// An X11 menu, tooltip or dropdown is placed by its client and owned by
/// it: `size_window` refuses an override-redirect surface, and nothing in
/// the layout moves one. So a resize border eight pixels outside a Steam or
/// GTK menu is a cursor promising a drag that cannot happen, and a press
/// there starts a `ResizeGrab` that does nothing when it should have
/// dismissed the menu. The single-pass walk made that worse before this
/// gate: the phantom border beat a lower window's real titlebar.
///
/// Covering is untouched, which is the half that was always right — the
/// menu is drawn and a press on it is the menu's.
#[test]
fn an_unmanaged_pane_occludes_but_offers_no_chrome() {
    let lower = Stacked::window((100, 100), (400, 300));
    let menu = Stacked::window((300, 20), (200, 100)).unmanaged();
    let stack = [menu, lower];

    // The same point as the halo case above: 4px below the menu's bottom
    // edge, over the lower window's titlebar.
    let button = (470.0, 124.0);
    assert_eq!(
        menu.hit(button),
        PaneHit::Miss,
        "a menu has no resize border to offer, inside or out"
    );
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(button))),
        Some(Chrome::Frame),
        "so the titlebar under it is pressable, phantom border or not"
    );

    // And on the menu itself: covered, and the press is the client's.
    let on_menu = (400.0, 60.0);
    assert_eq!(menu.hit(on_menu), PaneHit::Client);
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(on_menu))),
        None,
        "a press on a menu is the menu's, which is what occluding means"
    );

    // The inside half of the border it would otherwise have offered is the
    // client's too, rather than a resize of something nothing may resize.
    let edge = (400.0, 116.0);
    assert_eq!(
        Stacked::window((300, 20), (200, 100)).hit(edge),
        PaneHit::Chrome(Chrome::Resize(ResizeEdge::Bottom)),
        "a managed window of the same shape would offer this"
    );
    assert_eq!(menu.hit(edge), PaneHit::Client);
}

/// **An invisible pane offers nothing and covers nothing** — the one kind
/// that fails both of [`pane_hit_of`]'s questions at once.
///
/// Issue #127's review finding 1 at the level of the rule rather than of
/// the compositor. A pane between its close animation landing and its
/// client acting on the request is held at opacity zero by a transform
/// written `release: false`, for `CLOSING` plus the whole grace period. It
/// is still in the stack and its rectangle still contains the point; it is
/// simply not on screen.
///
/// **The `Halo` is what makes this two gates and not one.** Suppressing
/// only `covers` would turn the chrome this pane claims into
/// [`PaneHit::Halo`] — the weakest claim, but a claim that survives the
/// walk and wins wherever nothing lower paints. A closed window's titlebar
/// and resize border would go on being pressable over bare desktop, which
/// is a worse bug than the one being fixed because there is nothing on
/// screen to explain it. Both questions are therefore asked, and every
/// point is a `Miss`.
///
/// Contrast [`an_unmanaged_pane_occludes_but_offers_no_chrome`]: a menu
/// offers no chrome *and still covers*, because it is drawn. That is the
/// distinction — offering is about what a press would mean, covering is
/// about pixels, and this pane has no pixels.
#[test]
fn an_invisible_pane_offers_no_chrome_and_covers_nothing() {
    let lower = Stacked::window((100, 100), (400, 300));
    // Directly over the lower window, which is the situation a close
    // leaves behind once the layout has reflowed into the space.
    let closed = Stacked::window((100, 100), (400, 300)).invisible();
    let stack = [closed, lower];

    // On the lower window's titlebar, which the invisible pane's own
    // titlebar sits exactly on top of.
    let titlebar = (300.0, 110.0);
    assert_eq!(
        Stacked::window((100, 100), (400, 300)).hit(titlebar),
        PaneHit::Chrome(Chrome::Frame),
        "a visible pane of the same shape owns this point -- without which \
         the assertions below pass for want of a titlebar rather than for \
         want of a pane"
    );
    assert_eq!(
        closed.hit(titlebar),
        PaneHit::Miss,
        "an invisible pane's titlebar is not pressable"
    );
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(titlebar))),
        Some(Chrome::Frame),
        "so the press reaches the titlebar that is actually drawn there"
    );

    // The body, which is the click-to-focus case and the one that took the
    // keyboard with it.
    let body = (300.0, 250.0);
    assert_eq!(
        closed.hit(body),
        PaneHit::Miss,
        "and it occludes nothing, so the walk descends rather than \
         stopping at `PaneHit::Client`"
    );

    // The resize border, inside and out. Outside is the `Halo` this gate
    // exists to prevent.
    let inside = (300.0, 396.0);
    let outside = (300.0, 404.0);
    assert_eq!(
        Stacked::window((100, 100), (400, 300)).hit(outside),
        PaneHit::Halo(Chrome::Resize(ResizeEdge::Bottom)),
        "a visible pane of the same shape claims this as a halo, which is \
         the claim that must not survive being invisible"
    );
    assert_eq!(closed.hit(inside), PaneHit::Miss);
    assert_eq!(
        closed.hit(outside),
        PaneHit::Miss,
        "an edge nobody can see is an edge nobody can drag, and a halo \
         would have been honoured over bare desktop"
    );
}

/// A pane whose application has not arrived offers its frame and not its
/// border, and covers either way.
///
/// The `window.is_some()` half of [`chrome_offered`]: a frame around a
/// loading window has working buttons, which is the point of drawing one,
/// and there is nothing yet for a resize to resize. What it must not do is
/// let the window behind it take the press, which is issue #111 again with
/// a different pane on top.
#[test]
fn a_loading_pane_offers_its_frame_and_not_its_border() {
    let lower = Stacked::window((100, 100), (400, 300));
    let loading = Stacked::window((300, 20), (200, 100)).loading();
    let stack = [loading, lower];

    assert_eq!(
        loading.hit((400.0, 30.0)),
        PaneHit::Chrome(Chrome::Frame),
        "its titlebar is drawn and its buttons work"
    );
    assert_eq!(
        loading.hit((400.0, 116.0)),
        PaneHit::Client,
        "and its bottom border is not offered, so the point is simply \
         inside what it draws"
    );
    assert_eq!(
        loading.hit((470.0, 124.0)),
        PaneHit::Miss,
        "nor is the half of that border outside it"
    );
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit((400.0, 80.0)))),
        None,
        "a press on a loading window's body is its own, not the window \
         behind it"
    );
}

/// **The cursor is composed from the same walk, so an occluded border does
/// not promise a resize.**
///
/// #108's finding one altitude up: the pointer's shape is read off
/// [`claim_of`] rather than off any second hit test, so whatever the walk
/// declines to claim is a point the compositor says nothing about. Without
/// this the fix would be half-applied — presses landing correctly while the
/// pointer went on describing the resize that the press no longer performs,
/// which is exactly the symptom #108 was filed for.
#[test]
fn an_occluded_border_promises_no_resize_cursor() {
    let lower = Stacked::window((100, 100), (400, 300));
    let upper = Stacked::window((300, 20), (200, 100));
    let stack = [upper, lower];
    let cursor_at = |point| {
        claim_of(
            false,
            false,
            topmost_chrome(stack.iter().map(|pane| pane.hit(point))),
        )
        .cursor()
    };

    // Over the lower window's close button, under the upper window's halo:
    // the frame's own arrow, and not the resize the halo would have asked
    // for.
    assert_eq!(cursor_at((470.0, 124.0)), Some(CursorIcon::Default));
    assert_eq!(
        claim_of(false, false, halo_wins(&stack, (470.0, 124.0))).cursor(),
        Some(CursorIcon::NsResize),
        "which is the pointer #111's first fix drew over that button"
    );

    // Over the desktop above the upper window, where the halo is the whole
    // claim: the resize cursor, because the press really would resize.
    assert_eq!(cursor_at((400.0, 16.0)), Some(CursorIcon::NsResize));

    // And on the upper window's client, over the lower window's titlebar:
    // nothing at all, so the client's own cursor stands. This is #111's
    // point and the reason `chrome_under` is the only hit test.
    let covered = Stacked::window((60, 60), (400, 300));
    assert_eq!(
        claim_of(
            false,
            false,
            topmost_chrome([covered, lower].iter().map(|pane| pane.hit((300.0, 116.0)))),
        )
        .cursor(),
        None
    );
}

/// A point no window covers still descends, which is the case the fix must
/// not break: [`PaneHit::Miss`] and [`PaneHit::Client`] are both "no chrome
/// here" and only one of them may stop the walk.
#[test]
fn a_point_nothing_covers_still_descends() {
    let lower = Stacked::window((100, 100), (400, 300));
    let upper = Stacked::window((150, 20), (200, 100));
    let stack = [upper, lower];

    // 92px to the right of the upper window -- well past its border -- and
    // 10px down into the lower window's titlebar.
    let past = (450.0, 110.0);
    assert_eq!(upper.hit(past), PaneHit::Miss);
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(past))),
        Some(Chrome::Frame),
        "a window that is not there covers nothing, and the titlebar \
         beside it is still pressable"
    );
    assert_eq!(
        two_pass(&stack, past),
        Some(Chrome::Frame),
        "which the rule this replaced also got right -- only the covered \
         case differs"
    );

    // Bare desktop: every pane misses and the walk runs out.
    let desktop = (700.0, 700.0);
    assert_eq!(upper.hit(desktop), PaneHit::Miss);
    assert_eq!(lower.hit(desktop), PaneHit::Miss);
    assert_eq!(
        topmost_chrome(stack.iter().map(|pane| pane.hit(desktop))),
        None
    );
    assert_eq!(
        topmost_chrome(std::iter::empty::<PaneHit<Chrome>>()),
        None,
        "and a desktop with no windows on it at all"
    );
}

mod real_client;

/// **#100, the Wayland half: a menu near a screen edge was drawn off it.**
///
/// `new_popup` took the positioner as `_positioner` and dropped it, which
/// left Smithay's `get_geometry()` standing — anchor and gravity honoured,
/// `constraint_adjustment` ignored. This walks the arithmetic of the case
/// that produces: a window whose right edge is near the right edge of a
/// 1920-wide screen, and a context menu anchored at that edge opening
/// rightwards.
///
/// Both directions are asserted. The first assertion is the bug — the
/// placement the old code produced is *outside* the screen — and the
/// second is the fix. Without the first, a `popup_target` that returned
/// something absurdly large would pass the test by making every placement
/// look fine.
///
/// It is a unit test of arithmetic rather than of `place_popup`, because
/// `place_popup` needs a live `PopupSurface`, which needs a client. What
/// it does pin is the part that was wrong: the translation between the
/// compositor's coordinates and the positioner's, which is `popup_target`,
/// and the fact that we ask for the *unconstrained* geometry.
#[test]
fn a_menu_at_the_screen_edge_is_flipped_back_onto_it() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::{
        Anchor, ConstraintAdjustment, Gravity,
    };

    let screen = at(0, 0, 1920, 1080);
    // A window whose right edge is at x = 1900, twenty pixels short of the
    // screen's.
    let window = at(1400, 100, 500, 800);

    // What a toolkit sends for a menu hung off a control near that edge:
    // anchored on the right of a small widget, opening rightwards, and
    // allowed to flip on either axis if that does not fit.
    let positioner = PositionerState {
        rect_size: (200, 300).into(),
        anchor_rect: at(450, 200, 10, 10),
        anchor_edges: Anchor::Right,
        gravity: Gravity::Right,
        constraint_adjustment: ConstraintAdjustment::FlipX | ConstraintAdjustment::FlipY,
        ..PositionerState::default()
    };

    // No parent popups: this menu hangs off the toplevel itself.
    let parents = Point::<i32, Logical>::from((0, 0));
    let target = popup_target(screen, window.loc, parents);

    // The placement before the fix, in compositor coordinates.
    let unconstrained = positioner.get_geometry();
    let on_screen = |geometry: Rectangle<i32, Logical>| {
        Rectangle::new(window.loc + parents + geometry.loc, geometry.size)
    };
    assert!(
        !screen.contains_rect(on_screen(unconstrained)),
        "the test case has stopped reaching off the screen, so it no \
         longer pins anything: {:?}",
        on_screen(unconstrained)
    );

    // And after it.
    let constrained = positioner.get_unconstrained_geometry(target);
    assert!(
        screen.contains_rect(on_screen(constrained)),
        "a popup that was allowed to flip is still off the screen: {:?}",
        on_screen(constrained)
    );
    // Flipped rather than merely shrunk: the size the client asked for is
    // the size it gets, which is the difference between a menu with its
    // entries in it and a menu with a scrollbar.
    assert_eq!(constrained.size, positioner.rect_size);
}

/// A submenu is measured from its parent popup, not from the window.
///
/// `get_popup_toplevel_coords` is the second of the two translations in
/// `popup_target`, and it is the one with nothing else to catch it: a
/// first-level menu has a zero offset, so dropping the term entirely would
/// leave every test that uses one passing. The screen a submenu is
/// constrained against has to be moved by how far down the chain it hangs,
/// or the deeper it goes the more room it thinks it has.
#[test]
fn a_submenu_is_offset_by_the_chain_above_it() {
    let screen = at(0, 0, 1920, 1080);
    let root = Point::<i32, Logical>::from((1400, 100));
    let parents = Point::<i32, Logical>::from((250, 60));

    let direct = popup_target(screen, root, (0, 0).into());
    let nested = popup_target(screen, root, parents);

    assert_eq!(nested.loc, direct.loc - parents);
    assert_eq!(nested.size, screen.size);
    // A point that is the top-left of the screen in compositor
    // coordinates is the top-left of the target in either popup's own.
    assert_eq!(root + parents + nested.loc, screen.loc);
}

/// **The refusal path a stale serial takes.**
///
/// A client may ask for a popup grab with any serial it likes, including
/// one from an event that is long gone or one it never received. The
/// compositor's answer has to be "no" — `grab` declines and returns —
/// rather than an assertion or an unwrap, because there is nothing above a
/// compositor to restart it.
///
/// `may_grab` is the half of that decision that can be pinned without a
/// client: the seat answers three booleans about a device and this decides
/// whether a popup may take it. The other half — `grab_popup` returning
/// `Err` for a popup that is already mapped, orphaned, or not the topmost
/// — is Smithay's, and `grab` handles it by logging and returning; see the
/// `match` there.
#[test]
fn a_grab_held_by_a_stranger_is_refused() {
    // Nothing holds the device: the ordinary case, a menu opening while
    // the compositor is idle.
    assert!(may_grab(false, false, false));
    // This chain already holds it. A submenu opening inside its parent's
    // grab arrives here, and refusing it would break every nested menu.
    assert!(may_grab(true, true, false));
    assert!(may_grab(true, false, true));
    // Somebody else holds it -- one of Solium's own move or resize grabs,
    // or a serial this client made up. Declined.
    assert!(!may_grab(true, false, false));
}
/// **A drag owns the pointer, so the compositor stops describing what is
/// under it.**
///
/// A client that starts a drag gets a `DnDGrab` installed on the pointer,
/// so `PointerHandle::is_grabbed` is true for the whole gesture -- checked
/// against smithay 0.7's `selection/data_device/device.rs`, which calls
/// `set_grab` with it, and `input/pointer/mod.rs`, where `is_grabbed` is
/// `!matches!(guard.grab, GrabStatus::None)`. That is the machinery, and it
/// is why a drag needs no flag of its own: [`Solium::assert_cursor`]
/// already declines to recompute while the pointer is grabbed.
///
/// What it buys is that dragging a file across a window's edge does not
/// make the pointer offer a resize. The press that would perform that
/// resize cannot happen -- the button is already down and belongs to the
/// drag -- so a resize arrow there is #108's symptom again, arrived at from
/// a fourth direction, and it would flicker on and off along every edge the
/// drag crosses.
///
/// The `false` half is what makes this able to fail: the same call with no
/// grab clears the shape, so an `assert_cursor` that had lost its early
/// return would clear it in both.
#[test]
fn a_grabbed_pointer_keeps_the_shape_it_had() {
    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());

    // Where a border drag leaves the pointer: `input::pointer_button`
    // asserts the shape as it starts the grab, precisely so that it holds
    // for the drag. A DnD grab arrives at the same place by a different
    // road -- whatever was showing when the buttons went down.
    assert!(state.pointer.assert(Some(CursorIcon::NwseResize)));

    // Nothing is mapped, so the hit test under this point claims nothing
    // and the compositor's answer for it is "say nothing" -- which is a
    // *write*, and the one the grab has to suppress.
    let location = Point::<f64, Logical>::from((300.0, 300.0));
    assert_eq!(state.claim_under(location), Claim::Nothing);
    assert_eq!(state.claim_under(location).cursor(), None);

    state.assert_cursor(location, true);
    assert_eq!(
        state.pointer.showing(),
        CursorImageStatus::Named(CursorIcon::NwseResize),
        "a grab owns the pointer until it ends, so crossing anything \
         underneath must not change the shape"
    );

    state.assert_cursor(location, false);
    assert_eq!(
        state.pointer.showing(),
        CursorImageStatus::default_named(),
        "and the first motion after the grab ends hands the pointer back \
         to the ordinary hit test"
    );
}
/// **Issue #57's state machine: the icon is kept for exactly one drag.**
///
/// Needs a real `WlSurface`, and one cannot be conjured: smithay offers no
/// constructor, and `wl_surface`'s server-side user data type is private,
/// so `Client::create_resource` cannot name it either. A client has to ask
/// over the wire. That is the same conclusion `scale_resend` above reaches
/// for `ToplevelSurface`, and this fixture is deliberately its smaller
/// half: one global, one surface, no `xdg_shell`, no buffer, no output.
///
/// **Nothing here may build a Qt scene**, which is why no toplevel is
/// opened and none is needed. A window mapped inside a process that is
/// already holding a raw libwayland connection aborts the whole test
/// binary -- see the long note in `scale_resend`, which found that the hard
/// way. A bare `wl_surface` never reaches `new_toplevel`, so no decoration
/// is ever built for it.
mod drag_icon {
    use super::*;
    use smithay::reexports::wayland_server::Display;
    use std::os::unix::net::UnixStream;
    use wayland_client::protocol::{wl_compositor, wl_registry, wl_surface};
    use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle};

    /// The client side: `wl_compositor` and nothing else, because a
    /// surface is the whole of what is wanted.
    #[derive(Debug, Default)]
    struct Client {
        compositor: Option<wl_compositor::WlCompositor>,
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Client {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            (): &(),
            _conn: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            let wl_registry::Event::Global {
                name, interface, ..
            } = event
            else {
                return;
            };
            if interface == "wl_compositor" {
                state.compositor = Some(registry.bind(name, 1, qh, ()));
            }
        }
    }

    wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
    wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);

    /// Start a drag, drop it, start a second one, and destroy the surface
    /// under that one.
    ///
    /// One test rather than three because the fixture is the expensive
    /// part -- a display, a socket pair and a round trip -- and because
    /// each step is the next step's precondition: "cleared on drop" says
    /// nothing unless something was there to be cleared.
    #[test]
    fn an_icon_lasts_one_drag_and_outlives_neither_the_drop_nor_its_surface() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());

        let (server_side, client_side) =
            UnixStream::pair().expect("a socket pair for the test client");
        let served = display
            .handle()
            .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
            .expect("inserting the test client");
        let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
        let mut event_queue = conn.new_event_queue::<Client>();
        let qh = event_queue.handle();
        let mut client = Client::default();

        conn.display().get_registry(&qh, ());
        conn.flush().expect("flushing get_registry");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching get_registry");
        display
            .flush_clients()
            .expect("flushing the registry snapshot");
        // Safe to block: the server wrote the whole registry on the line
        // above and nothing but this thread drives it, so these bytes are
        // already in the kernel buffer. Same argument as `scale_resend`'s
        // one blocking read.
        event_queue
            .blocking_dispatch(&mut client)
            .expect("reading the registry snapshot");

        let compositor = client.compositor.clone().expect("wl_compositor bound");
        let asked = compositor.create_surface(&qh, ());
        conn.flush().expect("flushing create_surface");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching create_surface");

        // The compositor's own handle on the surface the client just made.
        // The protocol id is the same number on both sides of one
        // connection, which is what makes this lookup exact rather than a
        // search for "the only surface around".
        let icon: WlSurface = served
            .object_from_protocol_id(&display.handle(), asked.id().protocol_id())
            .expect("the compositor made a wl_surface for the request");

        // Taken once: every call below needs it, and it cannot be read
        // out of `state` in the same expression that borrows `state`
        // mutably.
        let seat = state.seat.clone();

        // A drag with no icon is ordinary -- a text selection dragged
        // inside one window often has none -- and must leave nothing
        // behind to be drawn.
        ClientDndGrabHandler::started(&mut state, None, None, seat.clone());
        assert!(
            state.dnd_icon().is_none(),
            "a drag the client chose not to illustrate draws nothing"
        );

        // The drag the issue is about.
        state.redraw = false;
        ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
        assert_eq!(
            state.dnd_icon().as_ref(),
            Some(&icon),
            "the icon is offered exactly once, at the start of the drag, \
             and keeping it is the whole of #57"
        );
        assert!(
            state.redraw,
            "the icon appears at a pointer that has not moved, so nothing \
             else on screen damages the region it is about to occupy"
        );

        // The buttons come up. `DnDGrab::unset` calls its own `drop`,
        // which calls this, so a cancelled or stolen grab arrives here
        // too -- checked against smithay 0.7's `dnd_grab.rs`.
        state.redraw = false;
        ClientDndGrabHandler::dropped(&mut state, None, true, seat.clone());
        assert!(
            state.dnd_icon.is_none(),
            "the drag is over, so the icon stops being drawn -- otherwise \
             it stays painted over the session that outlived it"
        );
        assert!(state.redraw, "and the frame that removes it has to happen");

        // A drop that nobody accepted ends the drag just as thoroughly.
        ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
        ClientDndGrabHandler::dropped(&mut state, None, false, seat.clone());
        assert!(state.dnd_icon.is_none());

        // **The client goes away mid-drag**, which never reaches `dropped`:
        // the grab is only unset when the buttons come up, and an
        // application that is gone will not be raising any. Destroying the
        // surface is the same road a disconnect takes -- every object the
        // client owned is destroyed -- and it is the deterministic half.
        ClientDndGrabHandler::started(&mut state, None, Some(icon.clone()), seat.clone());
        assert!(state.dnd_icon().is_some());
        asked.destroy();
        conn.flush().expect("flushing the surface destroy");
        display
            .dispatch_clients(&mut state)
            .expect("dispatching the surface destroy");
        assert!(!icon.alive(), "the fixture destroyed the surface");
        assert!(
            state.dnd_icon().is_none(),
            "a dead surface produces no render elements, so keeping one \
             is a drag icon that is never drawn and never cleared"
        );
        assert!(
            state.dnd_icon.is_none(),
            "and the reader clears the field rather than filtering it on \
             every frame for the rest of the session"
        );
    }
}

mod data_control;

/// **#362: every paste a Wayland client asks of an X11 one before a pass
/// waits for it, in order.** Two requests between passes, as a client pasting
/// asks for more than one type: one slot kept only the second, and the first
/// read was dropped. And with no XWayland to answer, the pass lets both go,
/// which closes their descriptors: the reader sees an empty selection at once
/// rather than waiting for one.
#[test]
fn every_paste_asked_for_before_a_pass_waits_for_it_in_order() {
    use smithay::wayland::selection::{SelectionHandler, SelectionTarget};
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    let display = smithay::reexports::wayland_server::Display::<Solium>::new()
        .expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let event_loop =
        smithay::reexports::calloop::EventLoop::<Solium>::try_new().expect("an event loop");

    let mut readers = Vec::new();
    for mime in ["text/plain;charset=utf-8", "text/plain"] {
        let (ours, theirs) = UnixStream::pair().expect("a pipe for the transfer");
        let seat = state.seat.clone();
        SelectionHandler::send_selection(
            &mut state,
            SelectionTarget::Clipboard,
            mime.to_owned(),
            std::os::fd::OwnedFd::from(ours),
            seat,
            &(),
        );
        readers.push(theirs);
    }
    let asked: Vec<&str> = state
        .pending_selection
        .iter()
        .map(|(_, mime, _)| mime.as_str())
        .collect();
    assert_eq!(
        asked,
        ["text/plain;charset=utf-8", "text/plain"],
        "a request asked for before the pass was lost, or they are out of order"
    );

    crate::xwayland::settle_selection(&mut state, &event_loop.handle());
    assert!(
        state.pending_selection.is_empty(),
        "the pass left a request waiting"
    );
    for mut reader in readers {
        let mut read = Vec::new();
        reader
            .read_to_end(&mut read)
            .expect("the transfer ends rather than hanging");
        assert!(read.is_empty(), "nothing can have been written with no X11");
    }
}

/// **#362: both loops serve what Wayland clients paste from X11.** The pass
/// that serves it needs a loop handle, so each backend calls it from its own
/// loop -- and only the nested one did, so on the hardware a paste from Steam
/// into Firefox was never answered. Asked of the source text, for the reason
/// `render.rs`'s drag-icon test gives: serving it needs a real XWayland.
#[test]
fn both_loops_serve_what_wayland_clients_paste_from_x11() {
    for (backend, source) in [
        (
            "tty.rs",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/tty.rs")),
        ),
        (
            "winit.rs",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/winit.rs")),
        ),
    ] {
        assert!(
            source.contains("crate::xwayland::settle_selection("),
            "{backend}'s loop never serves a paste from an X11 client"
        );
    }
}

/// Where a monitor tells its clients it is.
///
/// A screenshot tool, a recorder and the screen-sharing portal lay the
/// monitors out from `wl_output.geometry` and `zxdg_output_v1`'s
/// `logical_position`, and `grim` takes even one named monitor by capturing
/// every monitor that box overlaps and composing them where they say they
/// are. So a monitor that says it is somewhere it is not is a screenshot of
/// the wrong monitor. These tests read what a real client is sent.
mod outputs {
    use super::*;
    use smithay::output::{Mode, PhysicalProperties, Subpixel};
    use smithay::reexports::wayland_server::Display;
    use std::collections::HashMap;
    use std::os::unix::net::UnixStream;
    use wayland_client::protocol::{wl_output, wl_registry};
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy as _, QueueHandle};
    use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};

    /// A client that binds every monitor and asks where each one is, the
    /// way grim does.
    #[derive(Debug, Default)]
    struct Client {
        manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
        outputs: Vec<wl_output::WlOutput>,
        /// Each `wl_output`'s name, by its protocol id.
        names: HashMap<u32, String>,
        /// The last `wl_output.geometry` position, by the output's id.
        geometry: HashMap<u32, (i32, i32)>,
        /// The last `zxdg_output_v1.logical_position`, by the id of the
        /// `wl_output` it was asked for.
        logical: HashMap<u32, (i32, i32)>,
    }

    /// Where one monitor said it is: over `wl_output`, then over
    /// `xdg_output`, each `None` until it has said.
    type Told = (Option<(i32, i32)>, Option<(i32, i32)>);

    impl Client {
        /// What one monitor said, by name.
        fn told(&self, name: &str) -> Told {
            let id = self
                .names
                .iter()
                .find(|(_, each)| each.as_str() == name)
                .map(|(id, _)| *id);
            id.map_or((None, None), |id| {
                (
                    self.geometry.get(&id).copied(),
                    self.logical.get(&id).copied(),
                )
            })
        }
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for Client {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            (): &(),
            _conn: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            let wl_registry::Event::Global {
                name, interface, ..
            } = event
            else {
                return;
            };
            match interface.as_str() {
                // Version 4, for `name`, which is how grim's `-o` finds one.
                "wl_output" => state.outputs.push(registry.bind(name, 4, qh, ())),
                "zxdg_output_manager_v1" => state.manager = Some(registry.bind(name, 3, qh, ())),
                _ => {}
            }
        }
    }

    wayland_client::delegate_noop!(Client: ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);

    impl Dispatch<wl_output::WlOutput, ()> for Client {
        fn event(
            state: &mut Self,
            output: &wl_output::WlOutput,
            event: wl_output::Event,
            (): &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            let id = output.id().protocol_id();
            match event {
                wl_output::Event::Geometry { x, y, .. } => {
                    state.geometry.insert(id, (x, y));
                }
                wl_output::Event::Name { name } => {
                    state.names.insert(id, name);
                }
                _ => {}
            }
        }
    }

    /// The data is the id of the `wl_output` it describes.
    impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for Client {
        fn event(
            state: &mut Self,
            _xdg: &zxdg_output_v1::ZxdgOutputV1,
            event: zxdg_output_v1::Event,
            output: &u32,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let zxdg_output_v1::Event::LogicalPosition { x, y } = event {
                state.logical.insert(*output, (x, y));
            }
        }
    }

    /// A client connected and done with its first round trips: it has
    /// every monitor, and an `xdg_output` for each.
    struct Connected {
        conn: Connection,
        queue: EventQueue<Client>,
        client: Client,
    }

    impl Connected {
        fn new(display: &mut Display<Solium>, state: &mut Solium) -> Self {
            let (server_side, client_side) =
                UnixStream::pair().expect("a socket pair for the test client");
            display
                .handle()
                .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
                .expect("inserting the test client");
            let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
            let queue = conn.new_event_queue::<Client>();
            conn.display().get_registry(&queue.handle(), ());
            let mut connected = Self {
                conn,
                queue,
                client: Client::default(),
            };
            connected.round_trip(display, state);
            let qh = connected.queue.handle();
            let manager = connected
                .client
                .manager
                .clone()
                .expect("zxdg_output_manager_v1 bound");
            for output in connected.client.outputs.clone() {
                let id = output.id().protocol_id();
                let _ = manager.get_xdg_output(&output, &qh, id);
            }
            connected.round_trip(display, state);
            connected
        }

        /// Send what this client asked, let the server answer, read the
        /// answer. Blocking is safe for the reason `data_control` gives: the
        /// `sync` is answered by the server before the read, and nothing but
        /// this thread drives it.
        fn round_trip(&mut self, display: &mut Display<Solium>, state: &mut Solium) {
            self.conn.display().sync(&self.queue.handle(), ());
            self.conn.flush().expect("flushing the client's requests");
            display
                .dispatch_clients(state)
                .expect("dispatching the client's requests");
            display.flush_clients().expect("flushing to the clients");
            self.queue
                .blocking_dispatch(&mut self.client)
                .expect("reading the server's answer");
        }
    }

    wayland_client::delegate_noop!(Client: ignore wayland_client::protocol::wl_callback::WlCallback);

    /// A 1920x1080 monitor made the way both backends make one: a global,
    /// its location left at the origin, and mapped there for
    /// `place_outputs` to move.
    fn plugged(display: &Display<Solium>, state: &mut Solium, name: &str) -> Output {
        let output = Output::new(
            name.to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_string(),
                model: name.to_string(),
            },
        );
        let _global = output.create_global::<Solium>(&display.handle());
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            Some((0, 0).into()),
        );
        state.space.map_output(&output, (0, 0));
        output
    }

    /// `name` placed on `side` of `of`, the top or left edges lined up.
    fn beside(name: &str, side: crate::monitor::Side, of: &str) -> crate::monitor::Placement {
        crate::monitor::Placement {
            name: name.to_string(),
            at: None,
            beside: Some((side, of.to_string(), crate::monitor::Align::Start)),
            mode: crate::monitor::Wanted::default(),
            vrr: None,
            transform: None,
            enabled: true,
            primary: false,
            scale: crate::monitor::Scaling::Fixed(1.0),
        }
    }

    /// **The second monitor said it was at 0,0, wherever it was placed.**
    ///
    /// Seen on two 2560x1440 monitors side by side: the log said
    /// `placed monitor="DP-2" x=2560 y=0`, and `wayland-info` said both
    /// were at 0,0. `place_outputs` moved the monitor in the space and
    /// never told the `Output`, whose location is what both protocols send.
    /// So `grim -o DP-1` captured both monitors, composed them over each
    /// other, and handed back the second one's picture: the wallpaper and
    /// the bar, without the windows on the first. Without the change in
    /// `place_outputs`, fails at "a client is told".
    #[test]
    fn a_placed_output_tells_its_clients_where_it_is() {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let left = plugged(&display, &mut state, "left");
        let right = plugged(&display, &mut state, "right");
        // Nothing configured: the second goes to the right of the first,
        // which is what the hardware did.
        state.place_outputs();
        assert_eq!(
            state.space.output_geometry(&right).map(|at| at.loc),
            Some((1920, 0).into()),
            "the premise: the second monitor is placed to the right"
        );

        let mut client = Connected::new(&mut display, &mut state);
        assert_eq!(
            client.client.told("right"),
            (Some((1920, 0)), Some((1920, 0))),
            "a client is told the second monitor is where it was placed, \
             over wl_output and over xdg_output"
        );
        assert_eq!(client.client.told("left"), (Some((0, 0)), Some((0, 0))));

        // Placed again, as a configuration read again places it, with the
        // client already bound: it is told the new place, as the first is
        // when it moves.
        state.arrangement = crate::monitor::Arrangement::new(vec![beside(
            "right",
            crate::monitor::Side::Above,
            "left",
        )]);
        state.place_outputs();
        client.round_trip(&mut display, &mut state);
        let (above, below) = (
            state.space.output_geometry(&right).map(|at| at.loc),
            state.space.output_geometry(&left).map(|at| at.loc),
        );
        let point = |at: Option<Point<i32, Logical>>| at.map(|at| (at.x, at.y));
        assert_eq!(
            client.client.told("right"),
            (point(above), point(above)),
            "a monitor moved while a client is bound tells it where it went"
        );
        assert_eq!(client.client.told("left"), (point(below), point(below)));
        assert_ne!(above, below, "the premise: the two are not in one place");
    }
}
