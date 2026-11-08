//! The round trip of one batch and its answer between two processes, with
//! the protocol's own frames and encoding and a window manager keeping a
//! mirror: what an ask costs. Run with
//! `cargo run --release -p solium-wm-protocol --example round_trip`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "a measurement, not the compositor"
)]

use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

use solium_wm_protocol::frame::{self, Reader};
use solium_wm_protocol::mirror::Mirror;
use solium_wm_protocol::{
    Answer, Batch, Change, Command, Easing, Event, Insets, Message, Motion, Parentage, Place, Rect,
    Window,
};

fn window(id: u64) -> Window {
    Window {
        id,
        rect: Rect {
            x: 100.0,
            y: 200.0,
            w: 1280.0,
            h: 800.0,
        },
        insets: Insets {
            top: 36,
            right: 0,
            bottom: 0,
            left: 0,
        },
        title: "Mozilla Firefox — a page with a long title".to_owned(),
        focused: id == 0,
        monitor: "DP-1".to_owned(),
        modal: false,
        parent: Parentage::None,
        leaving: false,
        minimized: false,
        above: false,
        icon: None,
        app_id: "org.mozilla.firefox".to_owned(),
        min: None,
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

/// The window manager: keep the mirror, place every changed window.
fn window_manager(path: &str) {
    let mut socket = UnixStream::connect(path).expect("connects");
    let mut reader = Reader::default();
    let mut mirror = Mirror::new();
    loop {
        match reader.read_blocking(&mut socket) {
            Ok(Message::Batch(batch)) => {
                let changed: Vec<u64> = batch
                    .changes
                    .iter()
                    .filter_map(|change| match change {
                        Change::Window(window) => Some(window.id),
                        _ => None,
                    })
                    .collect();
                mirror.apply_all(batch.changes);
                let commands = changed
                    .iter()
                    .take(20)
                    .filter_map(|id| mirror.window(*id))
                    .map(|window| {
                        Command::Place(Place {
                            id: window.id,
                            rect: window.rect.clone(),
                            motion: Motion {
                                duration_ms: 240.0,
                                easing: Easing::OutCubic,
                            },
                            tile: true,
                            inside: None,
                            cramped: false,
                            moved: false,
                        })
                    })
                    .collect();
                let answer = Message::Answer(Answer {
                    serial: batch.serial,
                    handled: true,
                    commands,
                });
                frame::write(&mut socket, &answer).expect("answers");
            }
            _ => return,
        }
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p) as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--wm" {
        window_manager(&args[2]);
        return;
    }
    let path = std::env::temp_dir().join(format!("solium-wm-round-trip-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("binds");
    let path_text = path.to_string_lossy().into_owned();
    let mut child = std::process::Command::new(&args[0])
        .args(["--wm", &path_text])
        .spawn()
        .expect("starts the window manager");
    let (mut socket, _) = listener.accept().expect("accepts");
    let mut reader = Reader::default();
    for (label, windows, rounds) in [
        ("a key, nothing changed", 0u64, 20_000),
        ("1 window changed", 1, 20_000),
        ("50 windows changed", 50, 5_000),
    ] {
        let rows: Vec<Change> = (0..windows).map(|id| Change::Window(window(id))).collect();
        let mut times = Vec::with_capacity(rounds);
        let mut bytes = 0;
        for serial in 0..rounds as u64 {
            let start = Instant::now();
            let batch = Message::Batch(Batch {
                serial,
                changes: rows.clone(),
                events: vec![Event::Key("super+t".to_owned())],
            });
            let encoded = frame::encode(&batch);
            bytes = encoded.len();
            std::io::Write::write_all(&mut socket, &encoded).expect("sends");
            let answer = reader.read_blocking(&mut socket).expect("an answer");
            assert!(
                matches!(&answer, Message::Answer(answer) if answer.serial == serial),
                "not the answer to {serial}: {answer:?}"
            );
            times.push(start.elapsed());
        }
        times.sort();
        println!(
            "{label}: {bytes} bytes, {rounds} rounds: median {:?}, p99 {:?}, p99.9 {:?}, max {:?}",
            percentile(&times, 0.5),
            percentile(&times, 0.99),
            percentile(&times, 0.999),
            times[times.len() - 1]
        );
    }
    let _ = frame::write(&mut socket, &Message::Goodbye("done".to_owned()));
    let _ = child.wait();
    let _ = std::fs::remove_file(&path);
}
