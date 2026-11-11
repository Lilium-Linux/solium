//! The protocol between Solium and a window manager, generated from the
//! WIT in `wit/`, which is its one definition.
//!
//! A window manager is a separate program. It hears the desktop as a
//! [`Message::Batch`] of changes and events, and answers with commands; it
//! may also send commands of its own at any time. Every message is one
//! frame on a Unix stream socket ([`frame`]).
//!
//! # The encoding
//!
//! What is on the wire is protobuf. `proto/solium_wm.proto`, generated from
//! the same WIT, describes it in protobuf's own language, so a window
//! manager in any language with a protobuf generator is built from that
//! file, and this crate is the same thing for Rust:
//!
//! - a record is a message, its fields numbered from 1 in the WIT's order;
//! - a variant is a message with one `oneof`, its cases numbered from 1;
//! - an enum's cases are numbered from 0;
//! - unsigned integers are `uint32` and `uint64`, signed ones `sint32` and
//!   `sint64`, `f32` and `f64` are `float` and `double`, `list<u8>` is
//!   `bytes`, and a list of numbers is packed.
//!
//! Either side may be newer than the other. A field one does not know is
//! passed over, a field that was not sent is its type's default (0, false,
//! empty, none, a variant's first case), and a command, change or event in
//! a case one does not have is left out of its list and counted
//! ([`frame::Reader::unknown`]). So the WIT is only ever added to, at the
//! end of a record, a variant or an enum.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod client;
pub mod combo;
pub mod frame;
#[cfg(test)]
mod golden;
pub mod listen;
pub mod lock;
pub mod mirror;
pub mod present;
pub mod store;
pub mod touch;
mod wire;

pub use client::{Ask, compatible};
pub use wire::{DecodeError, Input, Kind, Wire};

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
        Answer, Batch, Change, Command, Configure, DocField, DocNode, DocNumbered, DocTable,
        Easing, Event, Focus, FocusCause, Hello, Input, Kept, Message, Monitor, Motion, Parentage,
        Place, Point, Rect, Spring, TimerDue, Window, WindowChange, Wire,
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
                keeps: vec![Kept {
                    name: "workspaces".to_owned(),
                    document: vec![
                        DocNode::Table(DocTable {
                            list: vec![1],
                            fields: vec![DocField {
                                name: "showing".to_owned(),
                                node: 2,
                            }],
                            numbered: vec![DocNumbered { key: 7, node: 1 }],
                        }),
                        DocNode::Text("2".to_owned()),
                        DocNode::Integer(-3),
                    ],
                }],
            }),
            Message::Batch(Batch {
                serial: u64::MAX,
                at: 1234.5,
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
                    Change::Cursor(Point { x: 1.5, y: -2.0 }),
                    Change::TextField(None),
                ],
                events: vec![
                    Event::Startup,
                    Event::Opened(1),
                    Event::Fullscreen(WindowChange {
                        id: 1,
                        entering: true,
                    }),
                    Event::Timers(vec![TimerDue { id: 4, last: false }]),
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
                    Command::Focus(Focus {
                        id: 1,
                        cause: FocusCause::Open,
                        warp: None,
                    }),
                    Command::Configure(Configure {
                        section: "idle".to_owned(),
                        settings: vec![DocNode::Nil],
                    }),
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
            assert_eq!(Input::read_whole::<Message>(&bytes), Ok((message, 0)));
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
        #[rustfmt::skip]
        let documented = [
            0x22, 14,               // `message`'s fourth case, `answer`: 14 bytes
            0x08, 0xac, 0x02,       // field 1, `serial`: 300
            0x10, 0x01,             // field 2, `handled`: true
            0x1a, 2, 0x70, 5,       // field 3, a command: its 14th case, `unplace`, 5
            0x1a, 3, 0xaa, 0x03, 0, // field 3, a command: its 53rd case, `quit`, nothing
        ];
        assert_eq!(
            message.to_bytes(),
            documented,
            "no motion and no decision, which are not sent at all"
        );
    }

    #[test]
    fn the_version_is_the_wit_packages() {
        assert_eq!(crate::VERSION, "0.2.0");
    }
}
