//! A Wayland client that asks the compositor questions and prints the answers.
//!
//! Solium cannot test its own protocol support from the inside, and "the global
//! is advertised" is a different claim from "a client that uses it gets the
//! right answers back". The difference has been the whole story of a week of
//! protocol work: a cursor that was advertised and invisible, a relative
//! pointer that was advertised and correct, a lock that was granted too early
//! to be heard.
//!
//! Real applications were borrowed for that verification — Firefox's own
//! `WAYLAND_DEBUG` log is a fine oracle — and it works right up until no
//! installed application happens to use the protocol you just added. Firefox
//! never binds `wp_presentation`, so nothing on this machine could tell whether
//! presentation feedback worked at all. This is that gap, filled.
//!
//!     cargo run -p wl-probe
//!
//! It maps one small window, waits for a few frames, and reports what came
//! back. Exit status is zero only if every check passed, so it can be a gate.

use std::os::unix::io::AsFd as _;
use std::time::Duration;

use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_registry, wl_shm, wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::{self, WpPresentation},
    wp_presentation_feedback::{self, WpPresentationFeedback},
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::XdgToplevel,
    xdg_wm_base::{self, XdgWmBase},
};

/// How many frames to ask about before reporting.
const FRAMES: usize = 5;

#[derive(Default)]
struct Probe {
    /// Every global the compositor advertised, with its version.
    globals: Vec<(String, u32)>,
    compositor: Option<WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<XdgWmBase>,
    presentation: Option<WpPresentation>,
    /// The clock id the compositor says its timestamps are on.
    clock: Option<u32>,
    surface: Option<WlSurface>,
    configured: bool,
    /// One entry per frame the compositor said it had presented.
    presented: Vec<Presented>,
    discarded: usize,
}

#[derive(Debug)]
struct Presented {
    /// Reassembled from the protocol's split seconds-hi/seconds-lo/nanos.
    when: Duration,
    refresh: Duration,
    sequence: u64,
    flags: u32,
}

impl Probe {
    fn has(&self, name: &str) -> bool {
        self.globals.iter().any(|(it, _)| it == name)
    }
}

fn main() {
    let connection = match Connection::connect_to_env() {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("wl-probe: no compositor to talk to: {err}");
            eprintln!("          set WAYLAND_DISPLAY to the socket Solium reported.");
            std::process::exit(2);
        }
    };
    let display = connection.display();
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    display.get_registry(&handle, ());

    let mut probe = Probe::default();
    // Two roundtrips: the first brings the globals, the second whatever binding
    // them produced -- the presentation clock arrives that way.
    for _ in 0..2 {
        if let Err(err) = queue.roundtrip(&mut probe) {
            eprintln!("wl-probe: the compositor stopped talking: {err}");
            std::process::exit(2);
        }
    }

    let mut failures = Vec::new();

    println!("globals: {}", probe.globals.len());
    for wanted in [
        "wl_compositor",
        "wl_shm",
        "xdg_wm_base",
        "wp_presentation",
        "wp_viewporter",
        "wp_fractional_scale_manager_v1",
        "xdg_activation_v1",
        "zwp_relative_pointer_manager_v1",
        "zwp_pointer_constraints_v1",
        "zwp_primary_selection_device_manager_v1",
        "zxdg_decoration_manager_v1",
        "zwlr_layer_shell_v1",
    ] {
        let version = probe
            .globals
            .iter()
            .find(|(it, _)| it == wanted)
            .map(|(_, version)| *version);
        match version {
            Some(version) => println!("  ok      {wanted} v{version}"),
            None => {
                println!("  MISSING {wanted}");
                failures.push(format!("{wanted} is not advertised"));
            }
        }
    }

    // Presentation feedback needs a window on screen to be about.
    if probe.has("wp_presentation") {
        match map_and_measure(&connection, &mut queue, &mut probe) {
            Ok(()) => {}
            Err(reason) => failures.push(reason),
        }
    }

    println!();
    if failures.is_empty() {
        println!("wl-probe: everything asked for was answered");
    } else {
        for failure in &failures {
            println!("wl-probe: FAIL — {failure}");
        }
        std::process::exit(1);
    }
}

/// Put a window up, ask about `FRAMES` frames, and check the answers.
fn map_and_measure(
    connection: &Connection,
    queue: &mut wayland_client::EventQueue<Probe>,
    probe: &mut Probe,
) -> Result<(), String> {
    let handle = queue.handle();
    let (Some(compositor), Some(shm), Some(wm_base), Some(presentation)) = (
        probe.compositor.clone(),
        probe.shm.clone(),
        probe.wm_base.clone(),
        probe.presentation.clone(),
    ) else {
        return Err("the compositor advertised globals it then refused to bind".to_owned());
    };

    println!();
    match probe.clock {
        // 1 is CLOCK_MONOTONIC. Anything else is legal but means every
        // timestamp below is on a clock this program is not reading.
        Some(1) => println!("presentation clock: 1 (CLOCK_MONOTONIC)"),
        Some(other) => println!("presentation clock: {other} — not CLOCK_MONOTONIC"),
        None => return Err("wp_presentation never said which clock it uses".to_owned()),
    }

    let surface = compositor.create_surface(&handle, ());
    let xdg = wm_base.get_xdg_surface(&surface, &handle, ());
    let toplevel = xdg.get_toplevel(&handle, ());
    toplevel.set_title("wl-probe".to_owned());
    surface.commit();
    probe.surface = Some(surface.clone());

    // Wait to be configured before attaching anything: a client may not attach
    // a buffer until it has been told its size once.
    //
    // `roundtrip` and not `blocking_dispatch`. The latter waits for an event
    // that may never come, and a probe that hangs is worse than a probe that
    // reports nothing -- it takes the shell it was run from with it. A
    // roundtrip sends a sync and waits for its reply, so it returns as long as
    // the compositor is alive at all.
    for turn in 0..50 {
        // Only if it is taking unusually long. A compositor that configures
        // promptly should say nothing; one that never does should say what it
        // is waiting for rather than looking hung.
        if turn == 10 {
            eprintln!("wl-probe: still waiting to be configured…");
        }
        settle(connection, queue, probe)?;
        if probe.configured {
            break;
        }
    }
    if !probe.configured {
        return Err("the compositor never configured the window".to_owned());
    }

    let buffer = solid_buffer(&shm, &handle, 320, 200);

    for _ in 0..FRAMES {
        // Ask about *this* frame before committing it. The callback is
        // per-commit, which is the whole point: it answers about one frame.
        presentation.feedback(&surface, &handle, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.damage(0, 0, i32::MAX, i32::MAX);
        surface.commit();
        // Long enough for a compositor that only draws on damage to have drawn
        // and, on real hardware, for the page flip to have happened.
        for _ in 0..20 {
            settle(connection, queue, probe)?;
            if probe.presented.len() + probe.discarded >= FRAMES {
                break;
            }
        }
    }

    println!(
        "frames asked about: {FRAMES}   presented: {}   discarded: {}",
        probe.presented.len(),
        probe.discarded
    );
    for frame in &probe.presented {
        println!(
            "  at {:?}  refresh {:?}  seq {}  flags {:#04b}",
            frame.when, frame.refresh, frame.sequence, frame.flags
        );
    }

    if probe.presented.is_empty() {
        return Err(format!(
            "asked about {FRAMES} frames and was told about none — \
             the global is advertised and answers nothing"
        ));
    }

    // Timestamps must go forward. Out of order means the compositor is
    // reporting something other than when the frame was shown.
    let ordered = probe
        .presented
        .windows(2)
        .all(|pair| pair[1].when >= pair[0].when);
    if !ordered {
        return Err("presentation timestamps went backwards".to_owned());
    }

    // A monotonic timestamp near zero means it is being measured from this
    // process rather than from boot, which is the mistake that looks fine
    // locally and breaks anything comparing against its own clock.
    if let Some(first) = probe.presented.first()
        && probe.clock == Some(1)
        && first.when < Duration::from_secs(60)
    {
        return Err(format!(
            "the first timestamp is {:?}, which is not CLOCK_MONOTONIC \
             — that clock counts from boot",
            first.when
        ));
    }

    Ok(())
}

/// Flush, wait for the compositor to answer, dispatch whatever arrived.
///
/// Bounded by construction: the sync it sends is answered by any live
/// compositor, so this cannot wait forever on a compositor that has simply
/// stopped having anything to say.
fn settle(
    connection: &Connection,
    queue: &mut wayland_client::EventQueue<Probe>,
    probe: &mut Probe,
) -> Result<(), String> {
    connection.flush().ok();
    queue
        .roundtrip(probe)
        .map_err(|err| format!("the compositor stopped talking: {err}"))?;
    std::thread::sleep(Duration::from_millis(20));
    Ok(())
}

/// A buffer with something in it, so the compositor has a frame to present.
fn solid_buffer(
    shm: &wl_shm::WlShm,
    handle: &QueueHandle<Probe>,
    width: i32,
    height: i32,
) -> WlBuffer {
    let stride = width * 4;
    let size = stride * height;
    let file = tempfile(size as usize);
    let pool = shm.create_pool(file.as_fd(), size, handle, ());
    pool.create_buffer(
        0,
        width,
        height,
        stride,
        wl_shm::Format::Argb8888,
        handle,
        (),
    )
}

/// An anonymous file of `size` bytes, filled with an opaque colour.
fn tempfile(size: usize) -> std::fs::File {
    use std::io::{Seek as _, Write as _};
    let mut file = tempfile_rs();
    // Opaque mid-blue, premultiplied — anything non-zero, so the compositor is
    // presenting a real frame rather than nothing.
    let pixel = [0x80u8, 0x50, 0x20, 0xff];
    let row: Vec<u8> = pixel.iter().copied().cycle().take(size).collect();
    file.write_all(&row).ok();
    file.flush().ok();
    file.seek(std::io::SeekFrom::Start(0)).ok();
    file
}

fn tempfile_rs() -> std::fs::File {
    // `O_TMPFILE` would be tidier; a named file in the runtime dir that is
    // unlinked immediately is portable and does the same thing.
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    let path = format!("{dir}/wl-probe-{}", std::process::id());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap_or_else(|err| panic!("wl-probe: could not make a buffer file: {err}"));
    std::fs::remove_file(&path).ok();
    file
}

impl Dispatch<wl_registry::WlRegistry, ()> for Probe {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        state.globals.push((interface.clone(), version));
        match interface.as_str() {
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(5), handle, ()));
            }
            "wl_shm" => state.shm = Some(registry.bind(name, 1, handle, ())),
            "xdg_wm_base" => state.wm_base = Some(registry.bind(name, version.min(6), handle, ())),
            "wp_presentation" => {
                state.presentation = Some(registry.bind(name, version.min(1), handle, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<WpPresentation, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &WpPresentation,
        event: wp_presentation::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock = Some(clk_id);
        }
    }
}

impl Dispatch<WpPresentationFeedback, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
                ..
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                state.presented.push(Presented {
                    when: Duration::new(seconds, tv_nsec),
                    refresh: Duration::from_nanos(u64::from(refresh)),
                    sequence: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                    flags: u32::from(flags),
                });
            }
            wp_presentation_feedback::Event::Discarded => state.discarded += 1,
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Probe {
    fn event(
        _: &mut Self,
        base: &XdgWmBase,
        event: xdg_wm_base::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // A ping unanswered is a client the compositor is entitled to kill.
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for Probe {
    fn event(
        state: &mut Self,
        xdg: &XdgSurface,
        event: xdg_surface::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            state.configured = true;
            if let Some(surface) = state.surface.as_ref() {
                surface.commit();
            }
        }
    }
}

delegate_noop!(Probe: ignore WlCompositor);
delegate_noop!(Probe: ignore WlSurface);
delegate_noop!(Probe: ignore wl_shm::WlShm);
delegate_noop!(Probe: ignore WlShmPool);
delegate_noop!(Probe: ignore WlBuffer);
delegate_noop!(Probe: ignore XdgToplevel);
