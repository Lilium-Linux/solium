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

use std::os::unix::io::{AsFd as _, AsRawFd as _};
use std::time::Duration;

use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_compositor::WlCompositor,
    wl_output::{self, WlOutput},
    wl_registry, wl_shm,
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::{self, WpPresentation},
    wp_presentation_feedback::{self, WpPresentationFeedback},
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::XdgToplevel,
    xdg_wm_base::{self, XdgWmBase},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

/// How tall a probe bar is, and how much of the screen it reserves.
const BAR: i32 = 48;

/// How many frames to ask about before reporting.
const FRAMES: usize = 5;

#[derive(Default)]
struct Probe {
    /// Every global the compositor advertised, with its version.
    globals: Vec<(String, u32)>,
    compositor: Option<WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<XdgWmBase>,
    layer_shell: Option<ZwlrLayerShellV1>,
    screencopy: Option<ZwlrScreencopyManagerV1>,
    presentation: Option<WpPresentation>,
    /// What a capture told us to allocate, and how it went.
    shot: Option<Shot>,
    /// The bars, kept alive past the layer check.
    ///
    /// Not tidiness: the capture check looks for one of them in the top rows
    /// of the image, which is the only way a client can tell an upside-down
    /// screenshot from a right-way-up one. Dropped at the end of `main` with
    /// everything else.
    bars_alive: Vec<(WlSurface, ZwlrLayerSurfaceV1)>,
    /// Every output, in the order advertised: the proxy, its name, and the
    /// size of its current mode.
    outputs: Vec<Screen>,
    /// The size each layer surface was configured to, by the output it named.
    bars: Vec<(String, u32, u32)>,
    /// The clock id the compositor says its timestamps are on.
    clock: Option<u32>,
    surface: Option<WlSurface>,
    configured: bool,
    /// One entry per frame the compositor said it had presented.
    presented: Vec<Presented>,
    discarded: usize,
}

/// A screen capture in progress.
#[derive(Debug, Default)]
struct Shot {
    /// From the `buffer` event: what to allocate.
    format: Option<u32>,
    width: u32,
    height: u32,
    stride: u32,
    /// Whether the compositor finished describing the buffer.
    described: bool,
    /// Whether it said the copy is done, and whether it gave up.
    ready: bool,
    failed: bool,
    /// The flags it reported, `Y_INVERT` among them.
    flags: u32,
}

/// One `wl_output`, as a client learns about it.
#[derive(Debug)]
struct Screen {
    output: WlOutput,
    /// From `wl_output.name`, which is version 4 — the connector name, and the
    /// same string `monitors` in the configuration uses.
    name: String,
    width: i32,
    height: i32,
    /// From `wl_output.scale`. Device pixels per logical one, as an integer —
    /// the protocol has no fractional form, and a compositor rounds up.
    scale: i32,
}

impl Screen {
    /// The monitor's size in *logical* pixels, which is what every protocol
    /// that positions anything speaks in.
    fn logical(&self) -> (i32, i32) {
        let scale = self.scale.max(1);
        (self.width / scale, self.height / scale)
    }
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
        "zwlr_screencopy_manager_v1",
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

    println!();
    println!("outputs: {}", probe.outputs.len());
    for screen in &probe.outputs {
        let name = if screen.name.is_empty() {
            // Version 4 or nothing. A compositor that does not send a name
            // leaves a client unable to say which monitor it wants, which is
            // the whole of putting a bar on the right screen.
            "(no name — wl_output is below version 4)"
        } else {
            &screen.name
        };
        let (logical_width, logical_height) = screen.logical();
        if screen.scale > 1 {
            println!(
                "  {name}  {}x{} at {}x  ({logical_width}x{logical_height} logical)",
                screen.width, screen.height, screen.scale
            );
        } else {
            println!("  {name}  {}x{}", screen.width, screen.height);
        }
    }
    if probe.outputs.iter().any(|screen| screen.name.is_empty()) {
        failures.push("an output arrived with no name".to_owned());
    }

    // A bar, a dock or a wallpaper is a layer surface, and which monitor one
    // lands on cannot be checked from inside the compositor.
    if probe.has("zwlr_layer_shell_v1") {
        match anchor_a_bar(&connection, &mut queue, &mut probe) {
            Ok(()) => {}
            Err(reason) => failures.push(reason),
        }
    }

    // A screenshot, which is the one check that looks at what the compositor
    // actually drew rather than at what it said.
    if probe.has("zwlr_screencopy_manager_v1") {
        // The whole monitor, then a region of it — the two entry points the
        // protocol has, and `grim` uses both.
        for region in [None, Some((200, 300, 400, 250))] {
            match take_a_shot(&connection, &mut queue, &mut probe, region) {
                Ok(()) => {}
                Err(reason) => failures.push(reason),
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

    // Held up so the compositor's own picture can be looked at: an exclusive
    // zone is a claim about the *other* windows, and no client can see those.
    if let Ok(seconds) = std::env::var("WL_PROBE_HOLD")
        && let Ok(seconds) = seconds.trim().parse::<u64>()
    {
        println!();
        println!("holding the bars up for {seconds}s — capture the compositor now");
        let until = std::time::Instant::now() + Duration::from_secs(seconds);
        while std::time::Instant::now() < until {
            if queue.roundtrip(&mut probe).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
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

/// Ask for a screenshot, and look at it.
///
/// The only check here that examines what the compositor *drew* rather than
/// what it said. Every other one reads an event; this one reads pixels, which
/// is the difference between "the protocol answers" and "there is a desktop in
/// the buffer".
///
/// Nothing that speaks this protocol is installed on the machine this was
/// written on — no grim, no wf-recorder, no portal — so this is not a
/// convenience. It is the only oracle there is.
///
/// `WL_PROBE_SHOT=<path>` writes the capture out as a binary PPM, because
/// "some pixels were not zero" and "that is my desktop" are different claims
/// and only one of them can be checked by a program.
fn take_a_shot(
    connection: &Connection,
    queue: &mut wayland_client::EventQueue<Probe>,
    probe: &mut Probe,
    region: Option<(i32, i32, i32, i32)>,
) -> Result<(), String> {
    let handle = queue.handle();
    let (Some(shm), Some(screencopy)) = (probe.shm.clone(), probe.screencopy.clone()) else {
        return Err("zwlr_screencopy_manager_v1 is advertised and would not bind".to_owned());
    };
    let Some(screen) = probe.outputs.first() else {
        return Err("no outputs to capture".to_owned());
    };
    let (output, name) = (screen.output.clone(), screen.name.clone());

    println!();
    probe.shot = Some(Shot::default());
    // No cursor: a screenshot with somebody's mouse in it is the thing
    // `overlay_cursor` exists to let you avoid, and asking for it here would
    // make the pixel check depend on where the pointer happens to be.
    let frame = match region {
        None => screencopy.capture_output(0, &output, &handle, ()),
        Some((x, y, w, h)) => screencopy.capture_output_region(0, &output, x, y, w, h, &handle, ()),
    };

    for turn in 0..50 {
        if turn == 10 {
            eprintln!("wl-probe: still waiting to be told what buffer to allocate…");
        }
        settle(connection, queue, probe)?;
        if probe.shot.as_ref().is_some_and(|shot| shot.described) {
            break;
        }
    }

    let (format, width, height, stride) = {
        let shot = probe.shot.as_ref().ok_or("the capture vanished")?;
        if !shot.described {
            return Err("the compositor never said what buffer a capture needs".to_owned());
        }
        (
            shot.format.ok_or("a capture with no format")?,
            shot.width,
            shot.height,
            shot.stride,
        )
    };
    println!("capture of {name}: {width}x{height}, stride {stride}, format {format}");

    if width == 0 || height == 0 {
        return Err("a capture of nothing".to_owned());
    }
    if stride < width * 4 {
        return Err(format!(
            "a stride of {stride} cannot hold {width} pixels of four bytes"
        ));
    }
    // What the buffer *should* be: the whole monitor in device pixels, or the
    // region asked for scaled the same way. A wrong size here is a capture of
    // something other than what was asked for, and every check below would
    // pass on it.
    let scale = probe
        .outputs
        .first()
        .map_or(1, |screen| screen.scale.max(1));
    let expected = match region {
        None => u32::try_from(screen_pixels(probe)).unwrap_or(0),
        Some((_, _, w, _)) => u32::try_from(w.max(0) * scale).unwrap_or(0),
    };
    if width != expected {
        return Err(format!(
            "a capture of {name} is {width} wide, expected {expected} device pixels"
        ));
    }

    // A pool the size the compositor asked for, and a buffer over it.
    let length = (stride * height) as usize;
    let file = tempfile_rs();
    file.set_len(length as u64)
        .unwrap_or_else(|err| panic!("wl-probe: could not size the capture pool: {err}"));
    let pool = shm.create_pool(file.as_fd(), length as i32, &handle, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::try_from(format).unwrap_or(wl_shm::Format::Xrgb8888),
        &handle,
        (),
    );

    frame.copy(&buffer);
    for turn in 0..100 {
        if turn == 20 {
            eprintln!("wl-probe: still waiting for the capture to be filled…");
        }
        settle(connection, queue, probe)?;
        let shot = probe.shot.as_ref().ok_or("the capture vanished")?;
        if shot.ready || shot.failed {
            break;
        }
    }

    let shot = probe.shot.as_ref().ok_or("the capture vanished")?;
    if shot.failed {
        return Err(format!("the compositor refused to capture {name}"));
    }
    if !shot.ready {
        return Err(format!(
            "asked for a capture of {name} and was never told it was done"
        ));
    }
    println!("  ok      filled, flags {:#04b}", shot.flags);

    // And now the part no event can tell us: is there a desktop in it?
    let pixels = std::fs::read(
        std::path::Path::new("/proc/self/fd").join(file.as_fd().as_raw_fd().to_string()),
    )
    .unwrap_or_default();
    let pixels = if pixels.len() >= length {
        pixels
    } else {
        // Reading back through /proc is a convenience, not a guarantee. Map
        // the pool instead if it did not work.
        return Err("could not read the capture back to look at it".to_owned());
    };

    if let Ok(path) = std::env::var("WL_PROBE_SHOT") {
        write_ppm(&path, width, height, stride, &pixels);
        println!("  wrote {path}");
    }

    // Not all zeros. An unbound framebuffer reads back as nothing at all, and
    // that is the failure this catches -- but only that one: the compositor
    // clears to a dark grey, so "not black" is true of an empty desktop too.
    let lit = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0)
        .count();
    if lit * 2 <= (width * height) as usize {
        return Err(format!(
            "a capture of {name} came back mostly zeros -- the copy went somewhere else"
        ));
    }

    // More than one colour in it. A capture that is a single flat colour is
    // either an empty clear or a copy of the wrong thing, and both look fine
    // to the check above.
    let mut seen = std::collections::HashSet::new();
    for row in 0..height as usize {
        let start = row * stride as usize;
        for x in (0..width as usize).step_by(7) {
            let at = start + x * 4;
            if let Some(pixel) = pixels.get(at..at + 3) {
                seen.insert([pixel[0], pixel[1], pixel[2]]);
            }
        }
        if seen.len() > 8 {
            break;
        }
    }
    println!("  ok      {} distinct colours sampled", seen.len());
    if seen.len() < 2 {
        return Err(format!(
            "a capture of {name} is one flat colour -- nothing was drawn into it"
        ));
    }

    // And the right way up, or in the right place. A bar was anchored to the
    // *top* of this monitor a moment ago, so its colour belongs in the top
    // rows of a whole-output capture and in neither band of a capture that
    // starts below it.
    //
    // The first of those is a check that cannot be done any other way: an
    // upside-down screenshot is perfectly legible and reads as a compositor
    // bug rather than a row-order one, which is exactly how it happened here.
    // The second catches a region offset that was ignored, which would hand
    // back the top-left corner whatever was asked for.
    fn band(pixels: &[u8], width: u32, height: u32, stride: u32, from: usize) -> usize {
        let mut found = 0;
        for row in from..(from + 8).min(height as usize) {
            let start = row * stride as usize;
            for x in (0..width as usize).step_by(5) {
                let Some(pixel) = pixels.get(start + x * 4..start + x * 4 + 3) else {
                    continue;
                };
                // The bar is a solid mid-blue; the desktop behind it is
                // near-black and the windows a dark slate. Blue clearly
                // dominant is the test, not an exact value — the buffer is
                // BGRX, so blue is byte zero.
                if usize::from(pixel[0]) > usize::from(pixel[2]) + 40 {
                    found += 1;
                }
            }
        }
        found
    }
    let (top, bottom) = (
        band(&pixels, width, height, stride, 2),
        band(&pixels, width, height, stride, height as usize - 10),
    );
    println!("  ok      bar pixels: {top} near the top, {bottom} near the bottom");
    match region {
        None if top <= bottom => Err(format!(
            "a capture of {name} has the top-anchored bar at the bottom -- the rows are \
             upside down ({top} bar pixels at the top, {bottom} at the bottom)"
        )),
        // A region that starts below the bar must not contain it. If it does,
        // the offset was ignored and this is the top of the screen.
        Some((_, y, _, _)) if y > 64 && top > 0 => Err(format!(
            "a capture of {name} from y={y} contains the bar, which is at the top of the \
             screen -- the region offset was ignored"
        )),
        _ => Ok(()),
    }
}

/// The first output's width in device pixels.
fn screen_pixels(probe: &Probe) -> i32 {
    probe.outputs.first().map_or(0, |screen| screen.width)
}

/// Write a captured buffer out as a binary PPM, so a person can look at it.
fn write_ppm(path: &str, width: u32, height: u32, stride: u32, pixels: &[u8]) {
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    for row in 0..height as usize {
        let start = row * stride as usize;
        for x in 0..width as usize {
            let at = start + x * 4;
            // Xrgb8888 is little-endian BGRX in memory.
            let (Some(b), Some(g), Some(r)) =
                (pixels.get(at), pixels.get(at + 1), pixels.get(at + 2))
            else {
                return;
            };
            out.extend_from_slice(&[*r, *g, *b]);
        }
    }
    let _ = std::fs::write(path, out);
}

/// Anchor a bar to the top of every monitor and check where each one landed.
///
/// This is the dock question, and it is one a compositor cannot answer about
/// itself. A layer surface names the output it wants; if the compositor puts it
/// on a different monitor, the configure comes back with the wrong width, and
/// that is measurable from out here and from nowhere else.
///
/// The exclusive zone is the other half, and it is *not* checkable from a
/// client: it is a claim about where the compositor puts everybody else's
/// windows. `WL_PROBE_HOLD=<seconds>` keeps the bars up so the compositor's own
/// frame can be captured and looked at.
fn anchor_a_bar(
    connection: &Connection,
    queue: &mut wayland_client::EventQueue<Probe>,
    probe: &mut Probe,
) -> Result<(), String> {
    let handle = queue.handle();
    let (Some(compositor), Some(shm), Some(layer_shell)) = (
        probe.compositor.clone(),
        probe.shm.clone(),
        probe.layer_shell.clone(),
    ) else {
        return Err("zwlr_layer_shell_v1 is advertised and would not bind".to_owned());
    };
    if probe.outputs.is_empty() {
        return Err("no outputs to put a bar on".to_owned());
    }

    println!();
    // One monitor, when asked for. A bar on every screen cannot tell "each
    // monitor's own work area" from "every monitor's work area" — the picture
    // looks the same either way. One bar on one screen can.
    let only = std::env::var("WL_PROBE_BAR").ok();
    // The *logical* width, because that is what a layer surface is configured
    // in. Comparing against the mode said a bar on a 2x monitor had landed on
    // another monitor, which is what this check is for and was wrong about:
    // the compositor was right and the expectation was 1x-only.
    let screens: Vec<(WlOutput, String, i32)> = probe
        .outputs
        .iter()
        .filter(|screen| only.as_deref().is_none_or(|only| only == screen.name))
        .map(|screen| {
            (
                screen.output.clone(),
                screen.name.clone(),
                screen.logical().0,
            )
        })
        .collect();
    if screens.is_empty() {
        return Err(format!(
            "no output called {:?} — this compositor has {:?}",
            only.unwrap_or_default(),
            probe
                .outputs
                .iter()
                .map(|screen| screen.name.clone())
                .collect::<Vec<_>>()
        ));
    }

    let mut surfaces = Vec::new();
    for (output, name, _) in &screens {
        let surface = compositor.create_surface(&handle, ());
        // Named explicitly. A surface that names no output is put wherever the
        // compositor's primary monitor is, which is the right default and the
        // wrong thing to test with: it cannot tell a correct answer from a
        // coincidence.
        let bar = layer_shell.get_layer_surface(
            &surface,
            Some(output),
            Layer::Top,
            format!("wl-probe-bar-{name}"),
            &handle,
            name.clone(),
        );
        bar.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        bar.set_size(0, BAR as u32);
        // The claim the rest of the desktop has to honour: this many pixels
        // off *this* monitor's work area, and no other monitor's.
        bar.set_exclusive_zone(BAR);
        surface.commit();
        surfaces.push((surface, bar, name.clone()));
    }

    for turn in 0..50 {
        if turn == 10 {
            eprintln!("wl-probe: still waiting for the bars to be configured…");
        }
        settle(connection, queue, probe)?;
        if probe.bars.len() >= screens.len() {
            break;
        }
    }

    // Configured, so they can be drawn. Something visible, because the point
    // of holding them up is to look at them.
    for (surface, _, _) in &surfaces {
        let buffer = solid_buffer(&shm, &handle, 3840, BAR);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage(0, 0, i32::MAX, i32::MAX);
        surface.commit();
    }
    settle(connection, queue, probe)?;

    let mut wrong = Vec::new();
    for (_, name, width) in &screens {
        match probe.bars.iter().find(|(on, _, _)| on == name) {
            Some((_, configured, height)) => {
                let ok = i64::from(*configured) == i64::from(*width);
                println!(
                    "  {} bar on {name}: configured {configured}x{height}, monitor is \
                     {width} logical wide",
                    if ok { "ok     " } else { "MISMATCH" }
                );
                if !ok {
                    // Anchored left and right, so the width it is given is the
                    // width of the monitor it is on. A different number means a
                    // different monitor.
                    wrong.push(format!(
                        "a bar that named {name} was configured {configured} wide, but {name} \
                         is {width} logical — it landed on another monitor"
                    ));
                }
                if i64::from(*height) != i64::from(BAR) {
                    wrong.push(format!(
                        "a bar on {name} asked for {BAR} tall and was configured {height}"
                    ));
                }
            }
            None => wrong.push(format!("a bar that named {name} was never configured")),
        }
    }

    // Held past this function: see `Probe::bars_alive`.
    probe
        .bars_alive
        .extend(surfaces.into_iter().map(|(surface, bar, _)| (surface, bar)));

    if let Some(first) = wrong.into_iter().next() {
        return Err(first);
    }
    Ok(())
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
            // Version 4 for `wl_output.name`, which is the connector name and
            // the only way a client can say *which* monitor it means.
            "wl_output" => {
                let output: WlOutput = registry.bind(name, version.min(4), handle, ());
                state.outputs.push(Screen {
                    output,
                    name: String::new(),
                    width: 0,
                    height: 0,
                    scale: 1,
                });
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), handle, ()));
            }
            "zwlr_screencopy_manager_v1" => {
                state.screencopy = Some(registry.bind(name, version.min(3), handle, ()));
            }
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
delegate_noop!(Probe: ignore ZwlrLayerShellV1);
delegate_noop!(Probe: ignore ZwlrScreencopyManagerV1);

impl Dispatch<ZwlrScreencopyFrameV1, ()> for Probe {
    fn event(
        state: &mut Self,
        _frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(shot) = state.shot.as_mut() else {
            return;
        };
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                shot.format = Some(format.into());
                shot.width = width;
                shot.height = height;
                shot.stride = stride;
                // Version 3 sends `buffer_done`; older versions do not, so a
                // buffer event is enough on its own to start allocating.
                shot.described = true;
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => shot.described = true,
            zwlr_screencopy_frame_v1::Event::Flags { flags } => shot.flags = flags.into(),
            zwlr_screencopy_frame_v1::Event::Ready { .. } => shot.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => shot.failed = true,
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for Probe {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(screen) = state
            .outputs
            .iter_mut()
            .find(|screen| &screen.output == output)
        else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => screen.name = name,
            // The *current* mode, not every mode the monitor offers -- an
            // output sends one of these per mode it supports.
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                screen.width = width;
                screen.height = height;
            }
            wl_output::Event::Scale { factor } => screen.scale = factor,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, String> for Probe {
    fn event(
        state: &mut Self,
        bar: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        on: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                // Acknowledged before anything is attached: the protocol says
                // a surface may not draw until it has agreed a size, and a
                // compositor is within its rights to kill a client that does.
                bar.ack_configure(serial);
                state.bars.push((on.clone(), width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => {
                eprintln!("wl-probe: the compositor closed the bar on {on}");
            }
            _ => {}
        }
    }
}
