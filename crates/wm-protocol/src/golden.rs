//! The encoding held to protobuf's own implementation, and to what lets
//! either side be the newer one.
//!
//! `proto/golden/` has messages written out in protobuf's text format and
//! the bytes `protoc` encodes them as (`make.sh` there). Each is built
//! here as the Rust value, and must read from those bytes and be written
//! as exactly them, so what this crate speaks is what every protobuf
//! library speaks.

use crate::wire::{self, Wire};
use crate::*;

fn golden(name: &str, bytes: &[u8], message: &Message) {
    assert_eq!(
        Input::read_whole::<Message>(bytes).as_ref(),
        Ok(&(message.clone(), 0)),
        "{name}: what protoc wrote reads as the message"
    );
    assert_eq!(
        message.to_bytes(),
        bytes,
        "{name}: and the message is written as protoc writes it"
    );
}

fn motion(duration_ms: f64, easing: Easing) -> Motion {
    Motion {
        duration_ms,
        easing,
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

fn answer() -> Message {
    Message::Answer(Answer {
        serial: 300,
        handled: true,
        commands: vec![Command::Unplace(5), Command::Quit],
        motion: None,
        decision: None,
    })
}

#[test]
fn an_answer_is_written_as_protoc_writes_it() {
    golden(
        "answer",
        include_bytes!("../proto/golden/answer.bin"),
        &answer(),
    );
}

#[test]
fn commands_of_every_shape_are_written_as_protoc_writes_them() {
    let commands = Message::Submit(vec![
        Command::Place(Place {
            id: 7,
            rect: rect(1.5, -2.0, 800.0, 600.0),
            motion: motion(180.0, Easing::OutCubic),
            tile: true,
            inside: Some(rect(0.0, 0.0, 10.0, 10.0)),
            cramped: false,
            moved: false,
        }),
        Command::Present(Present {
            id: 7,
            opacity: Some(0.0),
            matrix: Some(vec![1.0, 0.0, 0.0, 1.0]),
            z: 0.5,
            pivot: Pivot { x: 0.5, y: 0.5 },
            motion: motion(
                0.0,
                Easing::Bezier(Bezier {
                    x1: 0.25,
                    y1: 0.1,
                    x2: 0.25,
                    y2: 1.0,
                }),
            ),
            channels: vec![Channel {
                name: "x".to_owned(),
                motion: motion(90.0, Easing::Linear),
            }],
            ..Present::default()
        }),
        Command::Focus(Focus {
            id: 7,
            cause: FocusCause::Click,
            warp: Some(Warp {
                to: WarpTo::Window(WindowSpot {
                    id: 7,
                    x: 0.5,
                    y: 0.5,
                }),
                motion: None,
                interrupt: Interrupt::Picture,
            }),
        }),
        Command::Touch(TouchClaim {
            slot: -1,
            action: TouchAction::Resize(TouchResize {
                id: 7,
                edges: "top_left".to_owned(),
            }),
        }),
        Command::Press(PressOn {
            id: 7,
            press: Press::Keys(vec!["ctrl".to_owned(), "c".to_owned()]),
            cause: FocusCause::Script,
        }),
        Command::Decoration(None),
        Command::Decoration(Some(String::new())),
        Command::LockPeek(Some(2.5)),
        Command::KeepAbove(KeepAbove {
            id: 7,
            on: Some(false),
        }),
        Command::Spawn(vec!["foot".to_owned(), "--title=π".to_owned()]),
        Command::FileWrite(FileWrite {
            id: 3,
            path: "/tmp/x".to_owned(),
            data: vec![0, 1, 255],
            report: true,
        }),
        Command::Run(Run {
            id: 1,
            argv: vec!["ls".to_owned()],
            cwd: None,
            env: vec![
                EnvChange {
                    name: "A".to_owned(),
                    value: Some(String::new()),
                },
                EnvChange {
                    name: "B".to_owned(),
                    value: None,
                },
            ],
            timeout_ms: 1000.0,
            max_output: 65536,
            lines: true,
        }),
        Command::Surface(Surface {
            name: "bar".to_owned(),
            scene: "Bar.qml".to_owned(),
            layer: Layer::Top,
            on: Placement::Rect(PixelRect {
                x: -5,
                y: 0,
                w: 100,
                h: 30,
            }),
            properties: "{}".to_owned(),
            interactive: false,
            reserve: Insets {
                top: 30,
                ..Insets::default()
            },
            outside_click: OutsideClick {
                default: Outside::Pass,
                named: vec![OutsideNamed {
                    name: "menu".to_owned(),
                    outside: Outside::Swallow,
                }],
            },
            keyboard: KeyPolicy::NoBindings,
        }),
        Command::Configure(Configure {
            section: "idle".to_owned(),
            settings: vec![
                DocNode::Table(DocTable {
                    list: vec![0, 3],
                    fields: vec![DocField {
                        name: "after".to_owned(),
                        node: 1,
                    }],
                    numbered: vec![DocNumbered { key: -7, node: 2 }],
                }),
                DocNode::Integer(-3),
                DocNode::Nil,
                DocNode::Number(0.5),
                DocNode::Boolean(false),
                DocNode::Text(String::new()),
            ],
        }),
    ]);
    golden(
        "commands",
        include_bytes!("../proto/golden/commands.bin"),
        &commands,
    );
}

#[test]
fn a_batch_is_written_as_protoc_writes_it() {
    let batch = Message::Batch(Batch {
        serial: u64::MAX,
        at: 1234.5,
        changes: vec![
            Change::Window(Window {
                id: 1,
                rect: rect(10.0, 20.0, 640.0, 480.0),
                insets: Insets {
                    top: 30,
                    ..Insets::default()
                },
                title: "Öffnen".to_owned(),
                focused: true,
                monitor: "DP-1".to_owned(),
                parent: Parentage::Window(9),
                icon: Some(rect(1.0, 2.0, 3.0, 4.0)),
                app_id: "foot".to_owned(),
                min: Some(Size { w: 10, h: 10 }),
                style: "liquid".to_owned(),
                x11_type: Some("dialog".to_owned()),
                content_type: "none".to_owned(),
                pid: Some(0),
                ..Window::default()
            }),
            Change::WindowGone(4),
            Change::Stacking(vec![3, 1, 2]),
            Change::Monitors(vec![Monitor {
                name: "DP-1".to_owned(),
                area: rect(0.0, 32.0, 1920.0, 1048.0),
                whole: rect(0.0, 0.0, 1920.0, 1080.0),
                scale: 1.25,
                focused: false,
                primary: true,
                transform: "normal".to_owned(),
                off: false,
            }]),
            Change::Cursor(Point { x: 1.5, y: -2.0 }),
            Change::ShortcutsOwner(None),
            Change::TextField(Some(TextField {
                window: 1,
                caret: Some(rect(1.0, 1.0, 1.0, 10.0)),
                framed: true,
            })),
            Change::Locked(false),
            Change::Service(ServiceView {
                name: "audio".to_owned(),
                document: Vec::new(),
            }),
        ],
        events: vec![
            Event::Startup,
            Event::Key("super+t".to_owned()),
            Event::Fullscreen(WindowChange {
                id: 1,
                entering: true,
            }),
            Event::Timers(vec![
                TimerDue { id: 4, last: false },
                TimerDue { id: 5, last: true },
            ]),
            Event::Runs(vec![
                RunHeard::Stdout(RunLine {
                    id: 1,
                    line: b"hi".to_vec(),
                }),
                RunHeard::Done(RunEnded {
                    id: 1,
                    code: Some(0),
                    truncated: true,
                    ..RunEnded::default()
                }),
            ]),
            Event::Files(vec![
                FileHeard::Read(ReadEnded {
                    id: 2,
                    outcome: ResultBytes::Error("gone".to_owned()),
                }),
                FileHeard::Watched(WatchHeard {
                    id: 3,
                    change: FileChange::Removed,
                    path: "~/x".to_owned(),
                }),
            ]),
            Event::Sized(WindowSize {
                id: 1,
                size: Size { w: 640, h: 480 },
            }),
            Event::Focused(FocusMoved {
                id: 1,
                cause: FocusCause::Hover,
                changed: true,
            }),
        ],
    });
    golden("batch", include_bytes!("../proto/golden/batch.bin"), &batch);
}

/// The `.proto` file in the repository is what a window manager in another
/// language is built from, so it is the one the WIT generates, and its
/// diff in a pull request shows what the change does to the wire.
#[test]
fn the_proto_file_is_the_one_the_wit_generates() {
    let generated = include_str!(concat!(env!("OUT_DIR"), "/solium_wm.proto"));
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/proto/solium_wm.proto");
    if std::env::var_os("SOLIUM_WRITE_PROTO").is_some() {
        std::fs::write(path, generated).expect("proto/solium_wm.proto written");
    }
    let checked_in = std::fs::read_to_string(path).expect("proto/solium_wm.proto");
    assert!(
        checked_in == generated,
        "proto/solium_wm.proto is not what wit/ generates. Copy \
         target/debug/build/solium-wm-protocol-*/out/solium_wm.proto over it, or run this \
         test with SOLIUM_WRITE_PROTO=1, and read the diff: a field or a case that changed \
         its number breaks every window manager built before."
    );
}

/// The package for C# says the protocol's version in its handshake, and a
/// compositor refuses one that says another: it is the WIT's.
#[test]
fn the_package_for_c_sharp_says_the_protocols_version() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/csharp/Solium.Wm/Protocol.cs"
    );
    let package = std::fs::read_to_string(path).expect("sdk/csharp/Solium.Wm/Protocol.cs");
    assert!(
        package.contains(&format!("Version = \"{}\";", crate::VERSION)),
        "sdk/csharp/Solium.Wm/Protocol.cs does not say the protocol is {}",
        crate::VERSION
    );
}

/// A newer peer adds fields. One this version does not know is passed
/// over, whatever kind it is, and the rest is read.
#[test]
fn a_field_this_version_does_not_know_is_passed_over() {
    let Message::Answer(inner) = answer() else {
        unreachable!("an answer");
    };
    let mut bytes = Vec::new();
    wire::nested(&mut bytes, 4, |out| {
        wire::varint_field(out, 900, 7);
        inner.encode(out);
        wire::bytes_field(out, 901, b"later");
        wire::f64_field(out, 902, 1.5);
        wire::f32_field(out, 903, 1.5);
    });
    assert_eq!(Input::read_whole::<Message>(&bytes), Ok((answer(), 0)));
}

/// A newer window manager sends a command this compositor has no case
/// for. That command is left out and counted, so it can be said; the ones
/// around it are applied.
#[test]
fn a_command_this_version_does_not_have_is_left_out_and_counted() {
    let mut bytes = Vec::new();
    wire::nested(&mut bytes, 4, |out| {
        wire::varint_field(out, 1, 300);
        wire::varint_field(out, 2, 1);
        wire::message_field(out, 3, &Command::Unplace(5));
        // A command whose case is field 900.
        wire::nested(out, 3, |out| wire::bytes_field(out, 900, b"new"));
        // And one that holds, further in, a case this version has not: a
        // focus whose warp goes somewhere new. It cannot be applied as it
        // was meant, so it is left out whole.
        wire::nested(out, 3, |out| {
            wire::nested(out, 15, |out| {
                wire::varint_field(out, 1, 7);
                wire::nested(out, 3, |out| {
                    wire::nested(out, 1, |out| wire::varint_field(out, 900, 1));
                });
            });
        });
        wire::message_field(out, 3, &Command::Quit);
    });
    assert_eq!(Input::read_whole::<Message>(&bytes), Ok((answer(), 2)));
}

/// What a window manager does not set is not sent, and reads as its
/// type's default: a place with only an id and a size is a place that
/// does not animate and is not a tile.
#[test]
fn what_is_not_sent_is_its_default() {
    let mut bytes = Vec::new();
    wire::nested(&mut bytes, 5, |out| {
        wire::nested(out, 1, |out| {
            wire::nested(out, 13, |out| {
                wire::varint_field(out, 1, 9);
                wire::nested(out, 2, |out| {
                    wire::f64_field(out, 3, 640.0);
                    wire::f64_field(out, 4, 480.0);
                });
            });
        });
    });
    let (read, unknown) = Input::read_whole::<Message>(&bytes).expect("reads");
    assert_eq!(unknown, 0);
    assert_eq!(
        read,
        Message::Submit(vec![Command::Place(Place {
            id: 9,
            rect: rect(0.0, 0.0, 640.0, 480.0),
            motion: motion(0.0, Easing::Linear),
            tile: false,
            inside: None,
            cramped: false,
            moved: false,
        })])
    );
    // And a variant nobody set is its first case, not an error.
    let mut bytes = Vec::new();
    wire::nested(&mut bytes, 5, |out| {
        wire::nested(out, 1, |out| {
            wire::nested(out, 13, |out| {
                wire::nested(out, 3, |out| wire::nested(out, 2, |_| {}));
            });
        });
    });
    let (read, _) = Input::read_whole::<Message>(&bytes).expect("reads");
    let Message::Submit(commands) = read else {
        unreachable!("a submit");
    };
    assert!(
        matches!(commands.as_slice(), [Command::Place(place)] if place.motion.easing == Easing::Linear)
    );
}
