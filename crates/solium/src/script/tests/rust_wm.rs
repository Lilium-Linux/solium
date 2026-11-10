//! A window manager that is not the Lua one: the example a developer starts
//! from, `crates/wm-protocol/examples/tiny_wm.rs`, written in Rust against
//! the protocol crate and nothing of the compositor's. It runs here as the
//! program `wm.toml` names would, on a thread with its end of the socket,
//! and is driven by the compositor's own entry points.

use solium_wm_protocol::client::Client;
use solium_wm_protocol::{self as wire, Message, frame};

use super::*;

#[allow(
    dead_code,
    unreachable_pub,
    reason = "the example's `main` is its own program's"
)]
#[path = "../../../../wm-protocol/examples/tiny_wm.rs"]
mod tiny_wm;

fn tiny() -> Scripts {
    Scripts::played(|stream| {
        let _ = tiny_wm::serve(&Client::over(stream));
    })
    .expect("the example window manager answers `hello`")
}

/// Where each window of an answer was placed, in order.
fn placed(outcome: &Outcome) -> Vec<(u64, f64, f64)> {
    outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Place { id, rect, tile, .. } => {
                assert!(tile, "a tile the client is held inside");
                assert_eq!((rect.y, rect.h), (0.0, 900.0), "the screen's height");
                Some((*id, rect.x, rect.w))
            }
            _ => None,
        })
        .collect()
}

fn with_focus(mut snapshot: Snapshot, focused: u64) -> Snapshot {
    for window in &mut snapshot.windows {
        window.focused = window.id == focused;
    }
    snapshot
}

/// **A window manager written in Rust binds a key and places a window**:
/// its bindings are the compositor's to answer a key from, a window that
/// opens is placed where it says, its key does what it bound it to, and an
/// event it does not listen to is never sent to it.
#[test]
fn a_window_manager_in_rust_binds_a_key_and_places_a_window() {
    let mut scripts = tiny();
    let bound: Vec<(String, Option<String>)> = scripts
        .bindings()
        .into_iter()
        .map(|binding| (binding.combo, binding.note))
        .collect();
    assert!(
        bound.contains(&(
            "super+q".to_owned(),
            Some("close the focused window".to_owned())
        )),
        "{bound:?}"
    );
    assert!(scripts.has_binding("super+q"));
    assert!(!scripts.has_binding("super+t"));
    // It wrote its bindings in no order and `super+shift+e` in its own
    // spelling: the compositor finds each all the same, and names the key
    // back as it was written.
    assert!(scripts.has_binding("shift+super+e") && scripts.has_binding("Super+Shift+E"));
    assert_eq!(scripts.spelled("shift+super+e"), "super+shift+e");
    let outcome = scripts.key("shift+super+e", one_screen(&[]));
    assert!(
        outcome.handled && matches!(outcome.commands.as_slice(), [Command::Quit]),
        "{outcome:?}"
    );

    // The first window has the screen, the second half of it each.
    let outcome = scripts.opened(1, one_screen(&[1]));
    assert!(outcome.handled, "placed by the window manager: {outcome:?}");
    assert_eq!(placed(&outcome), [(1, 0.0, 1600.0)]);
    let outcome = scripts.opened(2, one_screen(&[1, 2]));
    assert_eq!(placed(&outcome), [(1, 0.0, 800.0), (2, 800.0, 800.0)]);

    // Its keys: the keyboard moves along, and round; the focused window
    // is asked to close.
    let outcome = scripts.key("super+j", with_focus(one_screen(&[1, 2]), 2));
    assert!(outcome.handled);
    assert!(
        matches!(outcome.commands.as_slice(), [Command::Focus { id: 1, .. }]),
        "{outcome:?}"
    );
    let outcome = scripts.key("super+q", with_focus(one_screen(&[1, 2]), 2));
    assert!(outcome.handled);
    assert!(
        matches!(outcome.commands.as_slice(), [Command::Close { id: 2 }]),
        "{outcome:?}"
    );
    let outcome = scripts.closed(2, one_screen(&[1]));
    assert_eq!(placed(&outcome), [(1, 0.0, 1600.0)]);

    // It listens to three events. Any other is not a batch at all.
    let serial = scripts.serial;
    let outcome = scripts.relayout(one_screen(&[1]));
    assert!(outcome.commands.is_empty() && !outcome.handled);
    scripts.dragged(1, "motion", 10.0, 10.0, one_screen(&[1]));
    assert_eq!(scripts.serial, serial);
    assert!(!scripts.down());
}

/// **A reload leaves it as it was started**: with no configuration to read
/// again it says what it said at the start, so its keys are still its own
/// and it still hears a window open.
#[test]
fn a_window_manager_in_rust_answers_a_reload() {
    let mut scripts = tiny();
    scripts.opened(1, one_screen(&[1]));
    scripts
        .reload(std::path::Path::new("/nothing/it/reads"))
        .expect("reloaded");
    assert!(scripts.has_binding("super+shift+e"));
    let outcome = scripts.opened(2, one_screen(&[1, 2]));
    assert_eq!(placed(&outcome), [(1, 0.0, 800.0), (2, 800.0, 800.0)]);
}

/// **A window manager of another version of the protocol is refused**: the
/// compositor says which it speaks and which the window manager does, and
/// starts nothing on it.
#[test]
fn a_window_manager_of_another_version_is_refused() {
    let refused = Scripts::played(|stream| {
        let mut reader = frame::Reader::default();
        let Ok(Message::Hello(_)) = reader.read_blocking(&mut &stream) else {
            return;
        };
        let _ = frame::write(
            &mut &stream,
            &Message::HelloAnswer(wire::HelloAnswer {
                version: "9.4.0".to_owned(),
                commands: Vec::new(),
            }),
        );
    })
    .expect_err("refused");
    let said = format!("{refused:#}");
    assert!(
        said.contains("9.4.0") && said.contains(wire::VERSION),
        "{said}"
    );
}
