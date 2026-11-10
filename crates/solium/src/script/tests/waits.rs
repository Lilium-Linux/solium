//! How long the compositor waits for the window manager, and what it does
//! when the answer is late, with a window manager played by hand
//! ([`Hand`]): it keeps the session's time, where every other test here
//! waits for every answer.

use std::time::{Duration, Instant};

use solium_wm_protocol::{self as wire, Message};

use super::*;
use crate::wm::Waits;
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
    scripts.waits.ask = SHORT;
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
    scripts.waits.ask = wait;
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
    scripts.waits.ask = SHORT;
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
    scripts.waits.ask = SHORT;
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
    scripts.waits.ask = SHORT;
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
    scripts.waits.ask = SHORT;
    scripts.relayout(drawn_over_the_point(7));
    scripts.relayout(empty_snapshot());
    assert_eq!((hand.batch(), hand.batch()), (1, 2));
    let under = || wire::Query::WindowAt((50.0, 40.0, None));

    hand.write(&Message::Query((1, under())));
    scripts.hear_ready();
    assert_eq!(
        hand.read(),
        Message::Reply((1, wire::Reply::Window(Some(7)))),
        "asked while handling the first batch"
    );
    hand.answer(1, false, Vec::new());
    hand.write(&Message::Query((2, under())));
    scripts.hear_ready();
    assert_eq!(
        hand.read(),
        Message::Reply((2, wire::Reply::Window(None))),
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
        scripts.waits,
        Waits {
            ask: Duration::from_secs(2),
            load: Duration::from_secs(20),
            ..Waits::default()
        }
    );

    let answered_in_60_ms = |ask: Duration| {
        let (mut scripts, mut hand) = Hand::with(Vec::new());
        scripts.waits.ask = ask;
        let answering = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            hand.answer(1, false, quit());
            hand
        });
        let outcome = scripts.relayout(empty_snapshot());
        let _hand = answering.join();
        asked_to_quit(&outcome)
    };
    assert!(!answered_in_60_ms(Waits::default().ask));
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
    assert_eq!(scripts.waits, Waits::default());
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
