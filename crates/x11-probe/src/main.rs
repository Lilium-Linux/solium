//! An X11 client that asks the window manager for fullscreen and maximised,
//! and prints what it was given.
//!
//! The test binary cannot start XWayland, so the window-manager half of Solium
//! is checked from outside, the way `wl-probe` checks the Wayland half, and
//! no X11 tool installed on a developer's machine makes these requests: there
//! is `xprop` and `glxgears`, and neither asks for anything (#361).
//!
//!     DISPLAY=:1 cargo run -p x11-probe -- request
//!     DISPLAY=:1 cargo run -p x11-probe -- mapped
//!
//! `request` maps an ordinary 640x480 window and then asks, through the
//! `_NET_WM_STATE` client message a toolkit sends, for fullscreen, to leave
//! it, to be maximised, and to be restored. `mapped` maps a window the way a
//! Wine or Proton game started fullscreen does: no frame (Motif hints), at
//! exactly the first monitor's rectangle, with `_NET_WM_STATE_FULLSCREEN`
//! written into its own property before mapping; and then asks to leave, to
//! be maximised, and to be restored.
//!
//!     DISPLAY=:1 cargo run -p x11-probe -- focus
//!
//! `focus` maps two windows one after the other and checks that each one,
//! given the keyboard as it opens, holds the X server's own input focus too,
//! which is what tells an X11 application it has the keyboard (#399). The
//! first is ICCCM's Passive kind, which the window manager sets the focus on.
//! The second is Globally Active: it declines `WM_HINTS.input`, lists
//! `WM_TAKE_FOCUS`, and sets the focus itself when the window manager sends
//! it one, as the probe does.
//!
//!     DISPLAY=:1 cargo run -p x11-probe -- maximized-before-map
//!     DISPLAY=:1 cargo run -p x11-probe -- maximized-at-map
//!     DISPLAY=:1 cargo run -p x11-probe -- ask-maximized
//!
//! The three ways an application asks to be maximised by itself (#450):
//! the `_NET_WM_STATE` message sent before the window maps, sent at once
//! after mapping it, as Steam restoring its last state does, and sent once
//! the window has been shown. With `X11_PROBE_TILED=1` a tiling layout is
//! in charge and the window must stay in its tile, not maximised; without
//! it the window must be maximised, and restored by asking again.
//!
//! Each step waits `X11_PROBE_HOLD` milliseconds (1500 by default) for the
//! compositor to answer and to be captured, then prints the window's state
//! and rectangle. The exit status is zero only if every answer was the one
//! asked for.

use std::time::Duration;

use x11rb::connection::Connection as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, InputFocus,
    PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// `_NET_WM_STATE`'s actions, as EWMH numbers them.
const REMOVE: u32 = 0;
const ADD: u32 = 1;

/// The atoms the probe uses, interned once.
struct Atoms {
    state: Atom,
    fullscreen: Atom,
    horizontal: Atom,
    vertical: Atom,
    motif: Atom,
}

impl Atoms {
    fn new(conn: &RustConnection) -> Self {
        let atom = |name: &str| {
            conn.intern_atom(false, name.as_bytes())
                .expect("asking for an atom")
                .reply()
                .expect("an atom")
                .atom
        };
        Self {
            state: atom("_NET_WM_STATE"),
            fullscreen: atom("_NET_WM_STATE_FULLSCREEN"),
            horizontal: atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
            vertical: atom("_NET_WM_STATE_MAXIMIZED_VERT"),
            motif: atom("_MOTIF_WM_HINTS"),
        }
    }
}

/// A rectangle in root coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

/// What the window looks like from its own side.
#[derive(Debug)]
struct Seen {
    rect: Rect,
    fullscreen: bool,
    maximized: bool,
}

struct Probe {
    conn: RustConnection,
    root: Window,
    window: Window,
    atoms: Atoms,
    hold: Duration,
    failed: bool,
}

impl Probe {
    /// Wait for the compositor, then say what the window is.
    fn look(&self) -> Seen {
        std::thread::sleep(self.hold);
        let geometry = self
            .conn
            .get_geometry(self.window)
            .expect("asking the geometry")
            .reply()
            .expect("the geometry");
        let at = self
            .conn
            .translate_coordinates(self.window, self.root, 0, 0)
            .expect("asking the position")
            .reply()
            .expect("the position");
        let states: Vec<Atom> = self
            .conn
            .get_property(false, self.window, self.atoms.state, AtomEnum::ATOM, 0, 32)
            .expect("asking _NET_WM_STATE")
            .reply()
            .expect("_NET_WM_STATE")
            .value32()
            .map(Iterator::collect)
            .unwrap_or_default();
        Seen {
            rect: Rect {
                x: i32::from(at.dst_x),
                y: i32::from(at.dst_y),
                w: u32::from(geometry.width),
                h: u32::from(geometry.height),
            },
            fullscreen: states.contains(&self.atoms.fullscreen),
            maximized: states.contains(&self.atoms.horizontal)
                && states.contains(&self.atoms.vertical),
        }
    }

    /// Ask the window manager to change a state, as a toolkit does.
    fn ask(&self, action: u32, first: Atom, second: Atom) {
        let message = ClientMessageEvent::new(
            32,
            self.window,
            self.atoms.state,
            [action, first, second, 1, 0],
        );
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                message,
            )
            .expect("sending _NET_WM_STATE");
        self.conn.flush().expect("flushing");
    }

    /// Print a step and whether it is what was asked for.
    fn check(&mut self, step: &str, seen: &Seen, ok: bool) {
        println!(
            "{} {step}: {}x{} at {},{}, fullscreen {}, maximised {}",
            if ok { "ok  " } else { "FAIL" },
            seen.rect.w,
            seen.rect.h,
            seen.rect.x,
            seen.rect.y,
            seen.fullscreen,
            seen.maximized,
        );
        self.failed |= !ok;
    }
}

/// The first monitor's rectangle, as RandR reports it, or the root window's.
fn monitor(conn: &RustConnection, root: Window, screen: (u16, u16)) -> Rect {
    conn.randr_get_monitors(root, true)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| {
            let first = reply
                .monitors
                .iter()
                .find(|monitor| monitor.primary)
                .or_else(|| reply.monitors.first())?;
            Some(Rect {
                x: i32::from(first.x),
                y: i32::from(first.y),
                w: u32::from(first.width),
                h: u32::from(first.height),
            })
        })
        .unwrap_or(Rect {
            x: 0,
            y: 0,
            w: u32::from(screen.0),
            h: u32::from(screen.1),
        })
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "request".into());
    let hold = std::env::var("X11_PROBE_HOLD")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or(1500);
    let (conn, screen_number) = x11rb::connect(None).expect("connecting to DISPLAY");
    let screen = &conn.setup().roots[screen_number];
    let root = screen.root;
    let size = (screen.width_in_pixels, screen.height_in_pixels);
    let atoms = Atoms::new(&conn);
    let monitor = monitor(&conn, root, size);
    println!(
        "monitor {}x{} at {},{}",
        monitor.w, monitor.h, monitor.x, monitor.y
    );

    let mapped = match mode.as_str() {
        "request" => false,
        "mapped" => true,
        "focus" => {
            let failed = focus(&conn, root, monitor, Duration::from_millis(hold));
            std::process::exit(i32::from(failed));
        }
        "maximized-before-map" | "maximized-at-map" | "ask-maximized" => {
            let tiled = std::env::var("X11_PROBE_TILED").is_ok_and(|tiled| tiled == "1");
            let failed = maximize(
                conn,
                root,
                atoms,
                monitor,
                Duration::from_millis(hold),
                &mode,
                tiled,
            );
            std::process::exit(i32::from(failed));
        }
        other => panic!(
            "no such mode {other:?}: `request`, `mapped`, `focus`, `maximized-before-map`, \
             `maximized-at-map` or `ask-maximized`"
        ),
    };
    let rect = if mapped {
        monitor
    } else {
        Rect {
            x: monitor.x + 100,
            y: monitor.y + 100,
            w: 640,
            h: 480,
        }
    };

    let window = conn.generate_id().expect("a window id");
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        i16::try_from(rect.x).expect("x fits"),
        i16::try_from(rect.y).expect("y fits"),
        u16::try_from(rect.w).expect("width fits"),
        u16::try_from(rect.h).expect("height fits"),
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        // A colour nothing else on screen is, so a capture shows where the
        // window is without the probe drawing anything.
        &CreateWindowAux::new()
            .background_pixel(0x0033_66cc)
            .event_mask(EventMask::STRUCTURE_NOTIFY),
    )
    .expect("creating the window");
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        format!("x11-probe {mode}").as_bytes(),
    )
    .expect("naming the window");
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        b"x11-probe\0x11-probe\0",
    )
    .expect("classing the window");
    if mapped {
        // What Wine writes for a borderless window: flags = decorations, and
        // no decorations.
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms.motif,
            atoms.motif,
            &[2, 0, 0, 0, 0],
        )
        .expect("asking for no frame");
        // And its own `_NET_WM_STATE`, which EWMH has a client that is not
        // mapped yet write rather than ask for.
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms.state,
            AtomEnum::ATOM,
            &[atoms.fullscreen],
        )
        .expect("writing _NET_WM_STATE");
    }
    conn.map_window(window).expect("mapping the window");
    conn.flush().expect("flushing");

    let mut probe = Probe {
        conn,
        root,
        window,
        atoms,
        hold: Duration::from_millis(hold),
        failed: false,
    };

    let seen = probe.look();
    if mapped {
        let ok = seen.fullscreen && seen.rect == monitor;
        probe.check("mapped covering the monitor, fullscreen", &seen, ok);
    } else {
        let ok = !seen.fullscreen && seen.rect != monitor;
        probe.check("mapped as a window", &seen, ok);

        probe.ask(ADD, probe.atoms.fullscreen, 0);
        let seen = probe.look();
        let ok = seen.fullscreen && seen.rect == monitor;
        probe.check("asked for fullscreen", &seen, ok);
    }

    probe.ask(REMOVE, probe.atoms.fullscreen, 0);
    let seen = probe.look();
    let ok = !seen.fullscreen && seen.rect != monitor;
    probe.check("asked to leave fullscreen", &seen, ok);
    let windowed = seen.rect;

    probe.ask(ADD, probe.atoms.horizontal, probe.atoms.vertical);
    let seen = probe.look();
    let ok = seen.maximized && !seen.fullscreen;
    probe.check("asked to be maximised", &seen, ok);

    probe.ask(REMOVE, probe.atoms.horizontal, probe.atoms.vertical);
    let seen = probe.look();
    let ok = !seen.maximized && seen.rect == windowed;
    probe.check("asked to be restored", &seen, ok);

    std::process::exit(i32::from(probe.failed));
}

/// `focus`: see the module header. Returns whether a step failed.
fn focus(conn: &RustConnection, root: Window, monitor: Rect, hold: Duration) -> bool {
    let atom = |name: &str| {
        conn.intern_atom(false, name.as_bytes())
            .expect("asking for an atom")
            .reply()
            .expect("an atom")
            .atom
    };
    let protocols = atom("WM_PROTOCOLS");
    let take_focus = atom("WM_TAKE_FOCUS");
    let open = |name: &str, globally_active: bool| {
        let window = conn.generate_id().expect("a window id");
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            i16::try_from(monitor.x + 100).expect("x fits"),
            i16::try_from(monitor.y + 100).expect("y fits"),
            640,
            480,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .background_pixel(0x0033_66cc)
                .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::FOCUS_CHANGE),
        )
        .expect("creating a window");
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            format!("x11-probe focus {name}").as_bytes(),
        )
        .expect("naming the window");
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"x11-probe\0x11-probe\0",
        )
        .expect("classing the window");
        if globally_active {
            // WM_HINTS with only the input flag set, and input False.
            conn.change_property32(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_HINTS,
                AtomEnum::WM_HINTS,
                &[1, 0, 0, 0, 0, 0, 0, 0, 0],
            )
            .expect("declining WM_HINTS.input");
            conn.change_property32(
                PropMode::REPLACE,
                window,
                protocols,
                AtomEnum::ATOM,
                &[take_focus],
            )
            .expect("listing WM_TAKE_FOCUS");
        }
        conn.map_window(window).expect("mapping the window");
        conn.flush().expect("flushing");
        window
    };
    // Wait `hold`, taking the focus the way a Globally Active client does
    // when the window manager asks.
    let wait = || {
        let until = std::time::Instant::now() + hold;
        while std::time::Instant::now() < until {
            while let Some(event) = conn.poll_for_event().expect("reading events") {
                if let x11rb::protocol::Event::ClientMessage(message) = event
                    && message.type_ == protocols
                    && message.data.as_data32()[0] == take_focus
                {
                    let time = message.data.as_data32()[1];
                    println!("     asked to take the focus: window {:#x}", message.window);
                    conn.set_input_focus(InputFocus::PARENT, message.window, time)
                        .expect("taking the focus");
                    conn.flush().expect("flushing");
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let focused = || {
        conn.get_input_focus()
            .expect("asking the input focus")
            .reply()
            .expect("the input focus")
            .focus
    };

    let mut failed = false;
    for (name, globally_active) in [("passive", false), ("globally-active", true)] {
        let window = open(name, globally_active);
        wait();
        let has = focused();
        let ok = has == window;
        println!(
            "{} {name} window {window:#x} opened: the X input focus is on {has:#x}",
            if ok { "ok  " } else { "FAIL" },
        );
        failed |= !ok;
    }
    failed
}

/// The `maximized-*` and `ask-maximized` modes: see the module header.
/// Returns whether a step failed.
fn maximize(
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    monitor: Rect,
    hold: Duration,
    mode: &str,
    tiled: bool,
) -> bool {
    let window = conn.generate_id().expect("a window id");
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        i16::try_from(monitor.x + 100).expect("x fits"),
        i16::try_from(monitor.y + 100).expect("y fits"),
        800,
        500,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(0x0033_66cc)
            .event_mask(EventMask::STRUCTURE_NOTIFY),
    )
    .expect("creating the window");
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        format!("x11-probe {mode}").as_bytes(),
    )
    .expect("naming the window");
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        b"x11-probe\0x11-probe\0",
    )
    .expect("classing the window");
    let mut probe = Probe {
        conn,
        root,
        window,
        atoms,
        hold,
        failed: false,
    };
    if mode == "maximized-before-map" {
        probe.ask(ADD, probe.atoms.horizontal, probe.atoms.vertical);
    }
    probe.conn.map_window(window).expect("mapping the window");
    probe.conn.flush().expect("flushing");
    if mode == "maximized-at-map" {
        probe.ask(ADD, probe.atoms.horizontal, probe.atoms.vertical);
    }
    let mut seen = probe.look();
    if mode == "ask-maximized" {
        probe.check("mapped as a window", &seen, !seen.maximized);
        probe.ask(ADD, probe.atoms.horizontal, probe.atoms.vertical);
        seen = probe.look();
    }
    if tiled {
        let ok = !seen.maximized && !seen.fullscreen && seen.rect != monitor;
        probe.check(
            "asked to be maximised under a layout: in its tile",
            &seen,
            ok,
        );
        return probe.failed;
    }
    let ok = seen.maximized && !seen.fullscreen;
    probe.check("asked to be maximised while floating", &seen, ok);
    let maximised = seen.rect;
    probe.ask(REMOVE, probe.atoms.horizontal, probe.atoms.vertical);
    let seen = probe.look();
    let ok = !seen.maximized && seen.rect != maximised && seen.rect.w <= 800 && seen.rect.h <= 500;
    probe.check("asked to be restored", &seen, ok);
    probe.failed
}
