//! How long the compositor waits for the window manager, and what it does
//! when the answer is late, with a window manager played by hand
//! ([`Hand`]): it keeps the session's time, where every other test here
//! waits for every answer.

use std::time::{Duration, Instant};

use solium_wm_protocol::{self as wire, Message};

use super::*;
use crate::wm::Settings;
use crate::wm::link::tests::Hand;

/// A wait short enough to run out in a test, and long enough to be seen.
const SHORT: Duration = Duration::from_millis(5);

fn quit() -> Vec<wire::Command> {
    vec![wire::Command::Quit]
}

fn asked_to_quit(outcome: &Outcome) -> bool {
    matches!(outcome.commands.as_slice(), [Command::Quit])
}

/// **An answer in time is the event's own**: handed back to whoever told the
/// event, with nothing left for later.
#[test]
fn an_answer_in_time_is_handed_back_with_the_event() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    // Written before the batch it answers is: a socket keeps it.
    hand.answer(1, true, quit());
    let outcome = scripts.relayout(empty_snapshot());
    assert!(asked_to_quit(&outcome), "{outcome:?}");
    assert!(scripts.late.is_empty());
    assert_eq!(hand.batch(), 1);
}

/// **A late answer is still applied**: the event goes on with nothing, and
/// what the window manager asked for is kept for when it comes.
#[test]
fn an_answer_that_comes_late_is_kept_for_when_it_comes() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    scripts.settings.ask = SHORT;
    let outcome = scripts.relayout(empty_snapshot());
    assert!(outcome.commands.is_empty(), "nothing came: {outcome:?}");
    scripts.hear_ready();
    assert!(scripts.late.is_empty(), "and nothing has yet");

    hand.answer(1, false, quit());
    scripts.hear_ready();
    let late = scripts.late.pop_front().expect("the answer, kept");
    assert!(asked_to_quit(&late.outcome), "{late:?}");
    assert!(scripts.late.is_empty());
}

/// **A slow window manager costs a frame once, not a frame an event**:
/// while one answer is late nothing after it is waited for, and once the
/// window manager has caught up, the next is again.
#[test]
fn nothing_is_waited_for_while_the_window_manager_is_behind() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    let wait = Duration::from_millis(150);
    scripts.settings.ask = wait;
    let told = Instant::now();
    scripts.relayout(empty_snapshot());
    assert!(told.elapsed() >= wait, "the first is waited for");
    let told = Instant::now();
    scripts.relayout(empty_snapshot());
    scripts.relayout(empty_snapshot());
    assert!(
        told.elapsed() < wait,
        "nothing is, while the first is unanswered: {:?}",
        told.elapsed()
    );

    for serial in 1..=3 {
        hand.answer(serial, false, quit());
    }
    scripts.hear_ready();
    assert_eq!(scripts.late.len(), 3, "each applied when it came");
    hand.answer(4, false, quit());
    assert!(
        asked_to_quit(&scripts.relayout(empty_snapshot())),
        "caught up, it is waited for again"
    );
}

/// **Answers are kept in the order they came**, and the last word before a
/// reload or the end is waited for past the ones before it.
#[test]
fn the_last_word_is_waited_for_past_the_answers_before_it() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    scripts.settings.ask = SHORT;
    scripts.relayout(empty_snapshot());
    hand.answer(1, false, vec![wire::Command::Status("first".to_owned())]);
    hand.answer(2, false, quit());
    let outcome = scripts.save_stores_at_last(empty_snapshot());
    assert!(asked_to_quit(&outcome), "its own answer: {outcome:?}");
    assert_eq!(scripts.late.len(), 1);
    assert_eq!(
        scripts
            .late
            .front()
            .and_then(|late| late.outcome.status.as_deref()),
        Some("first"),
        "and the one before it, kept to be applied first"
    );
}

/// **A bound key unanswered in time is the window manager's all the
/// same**, so it is not typed into the focused client; an unbound one is
/// the client's. What the binding asks for is applied when it comes.
#[test]
fn a_bound_key_unanswered_in_time_is_not_the_focused_clients() {
    let bound = wire::Command::Declare(wire::Declarations {
        bindings: vec![wire::Binding {
            combo: "super+t".to_owned(),
            note: None,
            repeating: false,
            locked: false,
            inhibited: false,
        }],
        unbound: Vec::new(),
        unknown: Vec::new(),
        notes: Vec::new(),
    });
    let (mut scripts, mut hand) = Hand::with(vec![bound]);
    scripts.settings.ask = SHORT;
    assert!(scripts.key("super+t", empty_snapshot()).handled);
    assert!(!scripts.key("super+u", empty_snapshot()).handled);

    hand.answer(1, true, quit());
    hand.answer(2, false, Vec::new());
    scripts.hear_ready();
    assert_eq!(scripts.late.len(), 2);
    assert!(
        scripts
            .late
            .front()
            .is_some_and(|late| asked_to_quit(&late.outcome))
    );
}

/// **A wheel turn unanswered in time is taken as the last one was**: a
/// layout that scrolls with the wheel does not let one turn through to the
/// window under the pointer because its answer was a frame late.
#[test]
fn a_wheel_turn_unanswered_in_time_is_taken_as_the_last_one_was() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    scripts.settings.ask = SHORT;
    assert!(
        !scripts.scrolled(0.0, 1.0, empty_snapshot()).handled,
        "nobody has taken one yet"
    );
    hand.answer(1, false, Vec::new());
    scripts.hear_ready();
    hand.answer(2, true, Vec::new());
    assert!(scripts.scrolled(0.0, 1.0, empty_snapshot()).handled);
    assert!(
        scripts.scrolled(0.0, 1.0, empty_snapshot()).handled,
        "late, and taken as the last was"
    );
}

/// A snapshot with window `id` drawn over (50, 40).
fn drawn_over_the_point(id: u64) -> Snapshot {
    let mut snapshot = one_screen(&[id]);
    let drawn = Rectangle::<f64, Logical>::new((10.0, 20.0).into(), (100.0, 50.0).into());
    let slot = drawn.to_i32_round();
    for window in &mut snapshot.windows {
        window.drawn = Drawn {
            slot,
            frame: Frame {
                rect: drawn,
                ..Frame::real(slot)
            },
        };
    }
    snapshot
}

/// **A question is about the batch it was asked in**: the window manager
/// handles batches in order, so what it asks while the compositor has gone
/// on is answered from the desktop as that batch found it, not as it is
/// now.
#[test]
fn a_question_is_answered_from_the_desktop_its_batch_was_sent_with() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    scripts.settings.ask = SHORT;
    scripts.relayout(drawn_over_the_point(7));
    scripts.relayout(empty_snapshot());
    assert_eq!((hand.batch(), hand.batch()), (1, 2));
    let under = || {
        wire::Query::WindowAt(wire::WindowAt {
            x: 50.0,
            y: 40.0,
            passing: None,
        })
    };

    hand.write(&Message::Query(wire::Asked {
        serial: 1,
        query: under(),
    }));
    scripts.hear_ready();
    assert_eq!(
        hand.read(),
        Message::Reply(wire::Replied {
            serial: 1,
            reply: wire::Reply::Window(Some(7)),
        }),
        "asked while handling the first batch"
    );
    hand.answer(1, false, Vec::new());
    hand.write(&Message::Query(wire::Asked {
        serial: 2,
        query: under(),
    }));
    scripts.hear_ready();
    assert_eq!(
        hand.read(),
        Message::Reply(wire::Replied {
            serial: 2,
            reply: wire::Reply::Window(None),
        }),
        "asked while handling the second"
    );
}

/// **`wm.ask_ms` is how long an answer is waited for**: a configuration
/// sets it, and an answer 60 ms in coming is late for the default and in
/// time for two seconds.
#[test]
fn the_wait_for_an_answer_is_the_configurations() {
    let (directory, scripts) = loaded(
        "solium-script-test-wm-waits",
        "sol.wm { ask_ms = 2000, load_ms = 20000 }",
    );
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(
        scripts.settings,
        Settings {
            ask: Duration::from_secs(2),
            load: Duration::from_secs(20),
            ..Settings::default()
        }
    );

    let answered_in_60_ms = |ask: Duration| {
        let (mut scripts, mut hand) = Hand::with(Vec::new());
        scripts.settings.ask = ask;
        let answering = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            hand.answer(1, false, quit());
            hand
        });
        let outcome = scripts.relayout(empty_snapshot());
        let _hand = answering.join();
        asked_to_quit(&outcome)
    };
    assert!(!answered_in_60_ms(Settings::default().ask));
    assert!(answered_in_60_ms(Duration::from_secs(2)));
}

/// **The shipped `config.lua` says what the compositor does before any
/// configuration has loaded**: its `wm` section is the defaults.
#[test]
fn the_shipped_configuration_waits_as_the_compositor_does_by_default() {
    let Some((directory, scripts)) =
        shipped_init_with_user("solium-script-test-wm-shipped", "return {}")
    else {
        return;
    };
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(scripts.settings, Settings::default());
}

/// **A window manager that has gone is told nothing more**, and each event
/// goes on with the compositor's own answer.
#[test]
fn a_window_manager_that_has_gone_is_told_nothing_more() {
    let (mut scripts, hand) = Hand::with(Vec::new());
    drop(hand);
    assert!(scripts.relayout(empty_snapshot()).commands.is_empty());
    assert!(scripts.down());
    assert!(!scripts.key("super+t", empty_snapshot()).handled);
}

/// **A window manager that says nothing for `wm.strikes` hard deadlines
/// running, with a batch waiting, has stopped.** Counted from the last
/// thing it said: a question is a sign of life, and one slow answer does
/// not make the batches behind it late in their turn.
#[test]
fn a_window_manager_that_says_nothing_with_a_batch_waiting_has_stopped() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    scripts.settings.ask = SHORT;
    scripts.settings.hard = Duration::from_secs(1);
    scripts.settings.strikes = 3;
    let before = Instant::now();
    scripts.relayout(empty_snapshot());
    scripts.relayout(empty_snapshot());
    let after = Instant::now();

    scripts.stalled(before + Duration::from_millis(2999));
    assert!(!scripts.down(), "three seconds have not passed");

    // It asks something two seconds in: it is there, and slow.
    hand.write(&Message::Query(wire::Asked {
        serial: 1,
        query: wire::Query::Styles,
    }));
    scripts.hear_ready();
    let asked = Instant::now();
    scripts.stalled(after + Duration::from_secs(3));
    assert!(!scripts.down(), "it said something since");

    scripts.stalled(asked + Duration::from_secs(3));
    assert!(scripts.down());
    assert_eq!(scripts.why_down(), Some("answered nothing for 3000 ms"));
}

/// **Nothing waiting, nothing to answer**: a window manager nobody has
/// told anything is not one that stopped, however long it is quiet.
#[test]
fn a_window_manager_with_nothing_to_answer_is_never_one_that_stopped() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    hand.answer(1, false, Vec::new());
    scripts.relayout(empty_snapshot());
    scripts.stalled(Instant::now() + Duration::from_secs(3600));
    assert!(!scripts.down());
}

/// **The shipped configuration loads with nothing of the user's own**: what
/// the compositor starts in place of a configuration that keeps stopping.
#[test]
fn the_shipped_configuration_loads_by_itself() {
    let scripts = Scripts::start_again(&Scripts::shipped_config(), true, Vec::new())
        .expect("the shipped configuration");
    assert!(scripts.shipped_only());
    assert_eq!(scripts.config(), crate::assets::lua().join("init.lua"));
    assert!(
        scripts.has_binding("super+shift+r"),
        "with its own way to reload"
    );
}

/// **An event the window manager does not listen to is not sent**: it is
/// answered at once as one nobody handled, with nothing on the socket. One
/// it does listen to is sent, and so are those that need no listening: a
/// key it bound, its own timers.
#[test]
fn an_event_nobody_listens_to_is_not_sent() {
    let (mut scripts, mut hand) = Hand::with(vec![wire::Command::Listen(vec![
        "closed".to_owned(),
        "no-such-event".to_owned(),
    ])]);
    scripts.settings.ask = SHORT;

    let outcome = scripts.relayout(empty_snapshot());
    assert!(
        outcome.commands.is_empty() && !outcome.handled,
        "{outcome:?}"
    );
    let outcome = scripts.opened(7, empty_snapshot());
    assert!(
        outcome.commands.is_empty() && !outcome.handled,
        "{outcome:?}"
    );
    assert_eq!(scripts.serial, 0, "neither was a batch");

    hand.answer(1, false, quit());
    let outcome = scripts.closed(7, empty_snapshot());
    assert!(asked_to_quit(&outcome), "{outcome:?}");
    hand.answer(2, true, Vec::new());
    let outcome = scripts.key("super+t", empty_snapshot());
    assert!(outcome.handled, "{outcome:?}");
    // A timer is a notice: it goes out with the pass, and its answer is
    // kept for when it comes.
    hand.answer(3, false, quit());
    scripts.timers_due(&[(4, true)], empty_snapshot());
    scripts.flush();
    scripts.hear_ready();
    let late = scripts.late.pop_front().expect("the timer's answer");
    assert!(asked_to_quit(&late.outcome), "{late:?}");

    let sent: Vec<wire::Event> = (0..3)
        .flat_map(|_| match hand.read() {
            Message::Batch(batch) => batch.events,
            other => panic!("not a batch: {other:?}"),
        })
        .collect();
    assert_eq!(
        sent,
        [
            wire::Event::Closed(7),
            wire::Event::Key("super+t".to_owned()),
            wire::Event::Timers(vec![wire::TimerDue { id: 4, last: true }]),
        ]
    );
}

/// **What a window manager listens to is what it last said, whole**: an
/// answer that names other events ends the first list, and the event
/// listened to before is no longer sent.
#[test]
fn a_new_list_of_events_takes_the_place_of_the_last() {
    let (mut scripts, mut hand) =
        Hand::with(vec![wire::Command::Listen(vec!["closed".to_owned()])]);
    hand.answer(
        1,
        false,
        vec![wire::Command::Listen(vec!["relayout".to_owned()])],
    );
    scripts.closed(7, empty_snapshot());
    assert_eq!(hand.batch(), 1);

    scripts.closed(8, empty_snapshot());
    assert_eq!(scripts.serial, 1, "no longer listened to, so not sent");
    hand.answer(2, false, quit());
    assert!(asked_to_quit(&scripts.relayout(empty_snapshot())));
    assert_eq!(hand.batch(), 2);
}

/// **Commands a window manager sends of its own accord are applied**: with
/// no batch asking for them, in the order they came among the late
/// answers, and what they declare is kept as an answer's is.
#[test]
fn commands_sent_unasked_are_applied_as_they_come() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    scripts.hear_ready();
    assert!(scripts.late.is_empty());

    hand.write(&Message::Submit(quit()));
    hand.write(&Message::Submit(vec![
        wire::Command::Status("ticked".to_owned()),
        wire::Command::Listen(vec!["closed".to_owned()]),
    ]));
    // Read as the event loop reads them, woken by the socket.
    let deadline = Instant::now() + Duration::from_secs(10);
    while scripts.late.len() < 2 {
        assert!(Instant::now() < deadline, "they never came");
        scripts.hear_ready();
    }
    let first = scripts.late.pop_front().expect("the first");
    assert!(asked_to_quit(&first.outcome), "{first:?}");
    let second = scripts.late.pop_front().expect("the second");
    assert_eq!(second.outcome.status.as_deref(), Some("ticked"));
    assert!(second.outcome.commands.is_empty(), "{second:?}");
    assert!(!scripts.down());

    // What the second one said it listens to holds from then on.
    scripts.relayout(empty_snapshot());
    assert_eq!(scripts.serial, 0, "`relayout` is no longer listened to");
}

/// **A configuration is sent what it has a listener for**: an event with
/// no `sol.on` for it is no batch at all, and it is one from the answer in
/// which a handler first listens to it.
#[test]
fn a_configuration_is_sent_only_what_it_listens_to() {
    let (directory, mut scripts) = loaded(
        "listens-as-it-goes",
        r#"
        sol.on("close", function(id)
            sol.status("closed " .. id)
            sol.on("layout", function() sol.status("laid out") end)
        end)
        "#,
    );
    let start = scripts.serial;
    let outcome = scripts.relayout(empty_snapshot());
    assert_eq!(outcome.status, None);
    assert_eq!(scripts.serial, start, "nobody listens to it: not sent");
    assert!(!scripts.hears(&wire::Event::Dragged(wire::PointerMove {
        id: 1,
        phase: "motion".to_owned(),
        x: 0.0,
        y: 0.0,
    })));
    assert!(
        scripts.hears(&wire::Event::SaveStores),
        "the runtime's own, whatever the configuration listens to"
    );

    let outcome = scripts.closed(3, empty_snapshot());
    assert_eq!(outcome.status.as_deref(), Some("closed 3"));
    assert_eq!(scripts.serial, start + 1);

    let outcome = scripts.relayout(empty_snapshot());
    assert_eq!(outcome.status.as_deref(), Some("laid out"));
    assert_eq!(scripts.serial, start + 2);
    let _ = std::fs::remove_dir_all(directory);
}

/// **A window manager is told its first moment, with the desktop**: one
/// that listens to nothing is still sent `startup`, and the batch carries
/// every window and monitor there is, which it has heard of none of.
#[test]
fn a_window_manager_is_told_when_it_starts_and_what_is_there() {
    let (mut scripts, mut hand) = Hand::with(vec![wire::Command::Listen(Vec::new())]);
    hand.answer(1, false, quit());
    let outcome = scripts.started(one_screen(&[4, 9]));
    assert!(asked_to_quit(&outcome), "{outcome:?}");
    let Message::Batch(batch) = hand.read() else {
        panic!("not a batch");
    };
    assert_eq!(batch.events, [wire::Event::Startup]);
    let windows: Vec<u64> = batch
        .changes
        .iter()
        .filter_map(|change| match change {
            wire::Change::Window(window) => Some(window.id),
            _ => None,
        })
        .collect();
    assert_eq!(windows, [4, 9]);
    assert!(
        batch.changes.iter().any(
            |change| matches!(change, wire::Change::Monitors(monitors) if monitors.len() == 1)
        ),
        "{:?}",
        batch.changes
    );
}

/// The events of a batch, read off the socket.
fn events_of(hand: &mut Hand) -> (u64, Vec<wire::Event>) {
    match hand.read() {
        Message::Batch(batch) => (batch.serial, batch.events),
        other => panic!("not a batch: {other:?}"),
    }
}

fn finger(kind: &'static str, slot: i32, x: f64) -> TouchEvent {
    TouchEvent {
        kind,
        slot,
        x,
        y: 0.0,
        output: String::new(),
        target: "window",
        window: None,
        control: false,
        edges: None,
        time: 0,
        t: 0.0,
        velocity: (0.0, 0.0),
    }
}

fn listening(events: &[&str]) -> (Scripts, Hand) {
    Hand::with(vec![wire::Command::Listen(
        events.iter().map(|event| (*event).to_owned()).collect(),
    )])
}

/// **Notices go out together**: an event nothing waits on is not sent
/// where it happens. The ones of a pass are one batch, in their order, and
/// the compositor did not stop for any of them.
#[test]
fn the_notices_of_a_pass_are_one_batch() {
    let (mut scripts, mut hand) = listening(&["renamed", "touch"]);
    scripts.settings.ask = Duration::from_millis(150);
    let told = Instant::now();
    scripts.timers_due(&[(4, false)], empty_snapshot());
    scripts.renamed(7, empty_snapshot());
    scripts.touch(finger("down", 0, 10.0), empty_snapshot());
    assert_eq!(scripts.serial, 0, "nothing is sent where it happens");
    scripts.flush();
    assert!(
        told.elapsed() < Duration::from_millis(150),
        "and nothing was waited for: {:?}",
        told.elapsed()
    );
    let (serial, events) = events_of(&mut hand);
    assert_eq!(serial, 1);
    assert_eq!(
        events.iter().map(wire::Event::case).collect::<Vec<_>>(),
        ["timers", "renamed", "touch"]
    );
}

/// **A slow window manager is sent fewer batches, never a backlog**: while
/// a batch of notices is unanswered the next ones wait, and a finger's or a
/// drag's later position takes the place of its earlier one. What is told
/// each time it happens is still told each time: a finger going down, a
/// drag beginning, a timer.
#[test]
fn notices_wait_for_the_batch_before_them_and_a_later_position_replaces_an_earlier() {
    let (mut scripts, mut hand) = listening(&["touch", "dragged", "renamed"]);
    scripts.touch(finger("down", 0, 0.0), empty_snapshot());
    scripts.flush();
    assert_eq!(events_of(&mut hand).0, 1);

    // The window manager is working on that one. Meanwhile:
    for x in [1.0, 2.0, 3.0] {
        scripts.touch(finger("motion", 0, x), empty_snapshot());
        scripts.touch(finger("motion", 1, x * 10.0), empty_snapshot());
    }
    scripts.dragged(7, "begin", 0.0, 0.0, empty_snapshot());
    scripts.dragged(7, "move", 5.0, 5.0, empty_snapshot());
    scripts.dragged(7, "move", 6.0, 6.0, empty_snapshot());
    scripts.timers_due(&[(4, false)], empty_snapshot());
    scripts.timers_due(&[(4, false)], empty_snapshot());
    scripts.renamed(7, empty_snapshot());
    scripts.renamed(7, empty_snapshot());
    scripts.touch(finger("up", 0, 3.0), empty_snapshot());
    scripts.touch(finger("down", 0, 9.0), empty_snapshot());
    scripts.touch(finger("motion", 0, 9.5), empty_snapshot());
    scripts.flush();
    assert_eq!(scripts.serial, 1, "they wait: the first is unanswered");

    hand.answer(1, false, Vec::new());
    scripts.hear_ready();
    let (serial, events) = events_of(&mut hand);
    assert_eq!(serial, 2, "its answer sends them");
    let told: Vec<String> = events
        .iter()
        .map(|event| match event {
            wire::Event::Touch(touch) => format!("{} {} {}", touch.kind, touch.slot, touch.x),
            wire::Event::Dragged(drag) => format!("drag {} {}", drag.phase, drag.x),
            other => other.case().to_owned(),
        })
        .collect();
    assert_eq!(
        told,
        [
            "motion 0 3",
            "motion 1 30",
            "drag begin 0",
            "drag move 6",
            "timers",
            "timers",
            "renamed",
            "up 0 3",
            "down 0 9",
            "motion 0 9.5",
        ]
    );
}

/// **An event that is waited for comes after the notices before it**, each
/// in a batch of its own, so the answer to the key is about the key.
#[test]
fn a_key_goes_after_the_notices_before_it() {
    let bound = wire::Command::Declare(wire::Declarations {
        bindings: vec![wire::Binding {
            combo: "super+t".to_owned(),
            ..wire::Binding::default()
        }],
        ..wire::Declarations::default()
    });
    let (mut scripts, mut hand) = Hand::with(vec![bound]);
    scripts.timers_due(&[(4, false)], empty_snapshot());
    hand.answer(1, false, vec![wire::Command::Status("timer".to_owned())]);
    hand.answer(2, true, quit());
    let outcome = scripts.key("super+t", empty_snapshot());
    assert!(
        outcome.handled && asked_to_quit(&outcome),
        "the key's own answer, waited for though the notices' batch was unanswered: {outcome:?}"
    );
    assert_eq!(
        scripts
            .late
            .front()
            .and_then(|late| late.outcome.status.as_deref()),
        Some("timer"),
        "and the timer's, kept to be applied before it"
    );
    assert_eq!(events_of(&mut hand).1.len(), 1);
    assert!(matches!(
        events_of(&mut hand).1.as_slice(),
        [wire::Event::Key(_)]
    ));
}

/// **Before a frame the batch in flight is waited for**, so what the
/// window manager asks for a finger is in the frame about to be drawn; and
/// **a slow window manager is not**: once a batch took longer than an
/// answer is waited for, the next frame does not stop for the next one.
#[test]
fn a_frame_waits_for_the_notices_of_a_window_manager_that_keeps_up() {
    let (mut scripts, mut hand) = Hand::with(Vec::new());
    let wait = Duration::from_millis(120);
    scripts.settings.ask = wait;

    // It answers at once: its answer is there to apply before the frame.
    scripts.timers_due(&[(4, false)], empty_snapshot());
    hand.answer(1, false, quit());
    scripts.before_frame();
    let late = scripts.late.pop_front().expect("answered before the frame");
    assert!(asked_to_quit(&late.outcome), "{late:?}");

    // It does not: the frame waits for it as long as an answer is waited
    // for, once.
    scripts.timers_due(&[(4, false)], empty_snapshot());
    let told = Instant::now();
    scripts.before_frame();
    assert!(told.elapsed() >= wait, "{:?}", told.elapsed());
    assert!(scripts.late.is_empty());
    let told = Instant::now();
    scripts.before_frame();
    assert!(
        told.elapsed() < wait / 2,
        "the same batch is not waited for twice: {:?}",
        told.elapsed()
    );
    hand.answer(2, false, quit());
    scripts.hear_ready();
    assert_eq!(scripts.late.len(), 1, "applied when it came");
    scripts.late.clear();

    // And the next frame does not wait for the next batch at all.
    scripts.timers_due(&[(4, false)], empty_snapshot());
    let told = Instant::now();
    scripts.before_frame();
    assert!(
        told.elapsed() < wait / 2,
        "a window manager that was slow costs the frame nothing: {:?}",
        told.elapsed()
    );
    // Until it keeps up again.
    hand.answer(3, false, Vec::new());
    scripts.hear_ready();
    scripts.late.clear();
    scripts.timers_due(&[(4, false)], empty_snapshot());
    hand.answer(4, false, quit());
    scripts.before_frame();
    assert_eq!(
        scripts.late.len(),
        1,
        "it answered in time, so it is waited for again"
    );
}
