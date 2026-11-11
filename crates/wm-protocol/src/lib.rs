//! The protocol between Solium and a window manager, generated from the
//! WIT in `wit/`, which is its one definition.
//!
//! A window manager is a separate program. It hears the desktop as a
//! [`Message::Batch`] of changes and events, and answers with commands; it
//! may also send commands of its own at any time. Every message is one
//! frame on a Unix stream socket ([`frame`]), its value in the encoding
//! below, so a window manager in any language can speak it from the WIT and
//! this page alone.
//!
//! # The encoding
//!
//! - `bool`: one byte, 0 or 1.
//! - unsigned integers: LEB128; signed ones: zigzag, then LEB128.
//! - `f32`, `f64`: little-endian IEEE 754.
//! - `char`: its scalar value, as a `u32`.
//! - `string`: its length in bytes, then its UTF-8.
//! - `list<T>`: its length, then each item.
//! - `option<T>`: 0 for none; 1, then the value.
//! - `tuple`, `record`: each element or field in order.
//! - `variant`, `enum`: the case's index in declaration order, then the
//!   case's payload, if it has one.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod client;
pub mod combo;
pub mod frame;
pub mod listen;
pub mod lock;
pub mod mirror;
pub mod present;
pub mod store;
pub mod touch;
mod wire;

pub use client::{Ask, compatible};
pub use wire::{DecodeError, Input, Wire};

#[allow(
    clippy::all,
    missing_docs,
    reason = "generated from the WIT by build.rs; the WIT carries the documentation"
)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

pub use generated::*;

/// The protocol's version: the WIT package's. A peer of another major
/// version is refused at the handshake.
pub const VERSION: &str = env!("SOLIUM_WM_PROTOCOL_VERSION");

#[cfg(test)]
mod tests {
    /// The documentation site's protocol reference is the WIT's comments,
    /// so a type, a message, a change, an event, a command, a query or a
    /// reply with none is one a window manager's author is told nothing
    /// about.
    #[test]
    fn every_word_of_the_protocol_says_what_it_means() {
        assert!(
            crate::UNDOCUMENTED.is_empty(),
            "no documentation in wit/ for: {:?}",
            crate::UNDOCUMENTED
        );
        assert!(
            crate::Event::CASES.len() > 40 && crate::Command::CASES.len() > 50,
            "the WIT was read"
        );
    }

    use crate::{
        Answer, Batch, Change, Command, DocNode, DocTable, Easing, Event, FocusCause, Hello, Input,
        Message, Monitor, Motion, Parentage, Place, Rect, Spring, Window, Wire,
    };

    fn window(id: u64) -> Window {
        Window {
            id,
            rect: Rect {
                x: 10.0,
                y: 20.0,
                w: 800.0,
                h: 600.0,
            },
            insets: crate::Insets {
                top: 36,
                right: 0,
                bottom: 0,
                left: 0,
            },
            title: "Терминал — fish".to_owned(),
            focused: id == 1,
            monitor: "DP-1".to_owned(),
            modal: false,
            parent: Parentage::Window(id + 1),
            leaving: false,
            minimized: false,
            above: false,
            icon: None,
            app_id: "foot".to_owned(),
            min: Some(crate::Size { w: 200, h: 100 }),
            max: None,
            cramped: false,
            shown: true,
            fullscreen: false,
            maximized: false,
            style: "liquid".to_owned(),
            client_side: false,
            blur_region: true,
            x11_type: None,
            accepts_input: true,
            class: None,
            instance: None,
            content_type: "none".to_owned(),
            pid: Some(4242),
        }
    }

    /// A message with something of every kind of WIT type in it reads back
    /// as it was written.
    #[test]
    fn every_kind_of_message_reads_back_as_it_was_written() {
        let messages = [
            Message::Hello(Hello {
                version: crate::VERSION.to_owned(),
                debug: true,
                keeps: vec![(
                    "workspaces".to_owned(),
                    vec![
                        DocNode::Table(DocTable {
                            list: vec![1],
                            fields: vec![("showing".to_owned(), 2)],
                            numbered: vec![(7, 1)],
                        }),
                        DocNode::Text("2".to_owned()),
                        DocNode::Integer(-3),
                    ],
                )],
            }),
            Message::Batch(Batch {
                serial: u64::MAX,
                changes: vec![
                    Change::Window(window(1)),
                    Change::WindowGone(9),
                    Change::Stacking(vec![3, 2, 1]),
                    Change::Monitors(vec![Monitor {
                        name: "DP-1".to_owned(),
                        area: Rect {
                            x: 0.0,
                            y: 32.0,
                            w: 2560.0,
                            h: 1408.0,
                        },
                        whole: Rect {
                            x: 0.0,
                            y: 0.0,
                            w: 2560.0,
                            h: 1440.0,
                        },
                        scale: 1.25,
                        focused: true,
                        primary: true,
                        transform: "normal".to_owned(),
                        off: false,
                    }]),
                    Change::Cursor((1.5, -2.0)),
                    Change::TextField(None),
                ],
                events: vec![
                    Event::Startup,
                    Event::Opened(1),
                    Event::Fullscreen((1, true)),
                    Event::Timers(vec![(4, false)]),
                ],
            }),
            Message::Answer(Answer {
                serial: 3,
                handled: false,
                commands: vec![
                    Command::Place(Place {
                        id: 1,
                        rect: Rect {
                            x: 0.0,
                            y: 0.0,
                            w: 1280.0,
                            h: 1440.0,
                        },
                        motion: Motion {
                            duration_ms: 240.0,
                            easing: Easing::Spring(Spring {
                                stiffness: 400.0,
                                damping: 30.0,
                                mass: 1.0,
                                initial_velocity: 0.0,
                                epsilon: 0.001,
                            }),
                        },
                        tile: true,
                        inside: None,
                        cramped: false,
                        moved: false,
                    }),
                    Command::Focus((1, FocusCause::Open, None)),
                    Command::Configure(("idle".to_owned(), vec![DocNode::Nil])),
                    Command::LockNow,
                ],
                motion: Some(Motion {
                    duration_ms: 120.0,
                    easing: Easing::Linear,
                }),
                decision: Some("{\"show\":false}".to_owned()),
            }),
            Message::Submit(vec![Command::Spawn(vec!["foot".to_owned()])]),
            Message::Goodbye("reloading".to_owned()),
        ];
        for message in messages {
            let bytes = message.to_bytes();
            assert_eq!(Input::read_whole::<Message>(&bytes), Ok(message));
        }
    }

    /// The bytes are the ones the documented encoding says, so a window
    /// manager written from the documentation reads what this crate writes.
    #[test]
    fn a_message_is_encoded_as_documented() {
        let message = Message::Answer(Answer {
            serial: 300,
            handled: true,
            commands: vec![Command::Unplace(5), Command::Quit],
            motion: None,
            decision: None,
        });
        let answer = 3; // `message`'s fourth case
        let unplace = 13; // `command`'s fourteenth case
        let quit = 52; // `command`'s last case
        assert_eq!(
            message.to_bytes(),
            [answer, 0xac, 0x02, 1, 2, unplace, 5, quit, 0, 0],
            "case, serial as LEB128, handled, two commands each a case and its payload, no \
             motion, no decision"
        );
    }

    #[test]
    fn the_version_is_the_wit_packages() {
        assert_eq!(crate::VERSION, "0.1.0");
    }
}
