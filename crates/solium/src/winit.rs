//! Winit backend: a window on a host compositor, for development.
//!
//! Never the path to a real session — that is DRM, later. This exists so the
//! compositor can be run and tested nested, without touching the developer's
//! live session.

use std::time::Duration;

use anyhow::{Context, Result};
use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_server::Display,
        winit::{
            dpi::LogicalSize,
            platform::{pump_events::PumpStatus, wayland::WindowAttributesExtWayland},
            window::WindowAttributes,
        },
    },
    utils::{Rectangle, Transform},
};

use crate::{
    capture, dev, layer, present, render,
    script::Scripts,
    state::{ClientState, Solium},
    synth,
};

/// A capture's path with a frame number in it: `frame.ppm` -> `frame-003.ppm`.
fn numbered(path: &std::path::Path, index: usize) -> std::path::PathBuf {
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{stem}-{index:03}{extension}"))
}

/// Frames a window must have been mapped for before a capture is honoured.
///
/// Counted from the first mapped window rather than from startup: a capture
/// taken before any client has drawn shows an empty compositor, which is the
/// misleading result this whole mechanism exists to avoid.
const CAPTURE_SETTLE_FRAMES: u32 = 30;

pub(crate) fn run() -> Result<()> {
    let mut event_loop: EventLoop<Solium> =
        EventLoop::try_new().context("creating the event loop")?;
    let display: Display<Solium> = Display::new().context("creating the wayland display")?;
    let display_handle = display.handle();

    let mut state = Solium::new(display_handle.clone());

    // The socket clients connect to. Named, not guessed: a client that resolves
    // WAYLAND_DISPLAY to empty gets the *default* socket, which on a developer
    // machine is their real session.
    let source = smithay::wayland::socket::ListeningSocketSource::new_auto()
        .context("binding a wayland socket")?;
    let socket_name = source.socket_name().to_string_lossy().into_owned();
    state.socket_name = socket_name.clone();

    event_loop
        .handle()
        .insert_source(source, |client_stream, _, state| {
            if let Err(err) = state
                .display_handle
                .insert_client(client_stream, std::sync::Arc::new(ClientState::default()))
            {
                tracing::warn!(?err, "rejecting a client we could not insert");
            }
        })
        .map_err(|e| anyhow::anyhow!("inserting the socket source: {e}"))?;

    event_loop
        .handle()
        .insert_source(
            smithay::reexports::calloop::generic::Generic::new(
                display,
                smithay::reexports::calloop::Interest::READ,
                smithay::reexports::calloop::Mode::Level,
            ),
            |_, display, state| {
                // SAFETY: Smithay requires this to be unsafe because the display
                // must not be dispatched re-entrantly. It is dispatched only
                // here, from a single-threaded event loop, so that holds.
                #[allow(unsafe_code)]
                let dispatched = unsafe { display.get_mut().dispatch_clients(state) };
                dispatched?;
                Ok(smithay::reexports::calloop::PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("inserting the display source: {e}"))?;

    // The app_id is stable and specific so the host compositor can be told
    // where to put this window and to leave the focus alone -- developing a
    // compositor should not steal focus from whatever is already running.
    let (mut backend, mut winit) = winit::init_from_attributes::<GlesRenderer>(
        WindowAttributes::default()
            .with_title("Solium (nested)")
            .with_name("solium-nested", "solium-nested")
            .with_inner_size(LogicalSize::new(1600.0, 900.0)),
    )
    .map_err(|e| anyhow::anyhow!("initialising the winit backend: {e}"))?;

    // The same hardware buffer sharing the hardware backend offers, so that a
    // client taking the fast path is exercised here rather than first
    // discovered on a TTY where nothing can be read.
    if std::env::var_os("SOLIUM_NO_DMABUF").is_none() {
        let formats: Vec<_> = backend
            .renderer()
            .egl_context()
            .dmabuf_texture_formats()
            .iter()
            .copied()
            .collect();
        tracing::info!(count = formats.len(), "advertising dmabuf formats");
        state.dmabuf_global = Some(
            state
                .dmabuf_state
                .create_global::<Solium>(&display_handle, formats),
        );
    }

    let size = backend.window_size();
    // The host's actual refresh, not an assumed 60: a client pacing itself to
    // the wrong number is a client that misses frames on purpose.
    let refresh = backend
        .window()
        .current_monitor()
        .and_then(|monitor| monitor.refresh_rate_millihertz())
        .and_then(|rate| i32::try_from(rate).ok())
        .unwrap_or(60_000);
    let mode = Mode { size, refresh };
    tracing::info!(refresh, "output mode");
    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Solium".into(),
            model: "Winit".into(),
        },
    );
    let _global = output.create_global::<Solium>(&display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));
    // Anchored surfaces are arranged against the output, so it has to exist
    // first; a shell connecting before this would be told a size of zero.
    layer::arrange(&output);

    // Scripts are loaded before the first frame so a mode can be triggered
    // immediately. A broken config leaves the compositor usable and unbound
    // rather than refusing to start.
    let config = Scripts::config_path();
    state.start_scripts(match Scripts::load(&config) {
        Ok(scripts) => Some(scripts),
        Err(err) => {
            tracing::error!(?err, config = %config.display(), "no scripts loaded");
            None
        }
    });

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    // Frames are captured a few ticks in, not on the first one: a client that
    // has just been configured has not drawn yet, and a capture of an empty
    // compositor is exactly the misleading result this exists to avoid.
    let mut capture = dev::capture_path();
    let mut settled = 0_u32;

    // Dev knobs, both documented in dev/README.md. They exist so the transform
    // path can be exercised and photographed without a human at the keyboard,
    // which is the only way this becomes a regression test later.
    let capture_at = dev::capture_at();
    let mut capture_remaining = dev::capture_frames();
    let capture_interval = dev::capture_interval();
    let mut capture_due = capture_at;
    let mut capture_index = 0_usize;
    let mut triggers = dev::triggers();
    let mut clicks = dev::clicks();
    let mut drags = dev::drags();
    triggers.reverse();
    clicks.reverse();

    // Frame pacing, reported periodically. Latency is the thing this
    // compositor will be judged on, and "it feels laggy" is not something that
    // can be acted on without a number.
    let mut frames = 0_u32;
    let mut window_started = Duration::ZERO;

    // Which host monitor the nested window landed on cannot be asked for at
    // startup: a Wayland client learns its output only when the host sends
    // wl_surface.enter, which is after the first frames. So it is reported once,
    // as soon as it is knowable.
    let mut monitor_reported = false;

    // Deliberately not exported into our own environment: clients need it in
    // *theirs*, and silently inheriting it is how a nested client ends up on the
    // developer's real session.
    tracing::info!(socket = %socket_name, "solium is up -- run clients with WAYLAND_DISPLAY set to this");

    loop {
        let status = winit.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
            }
            WinitEvent::Input(event) => crate::input::handle(&mut state, &output, event),
            WinitEvent::Focus(focused) => {
                tracing::info!(focused, "nested window focus changed");
            }
            _ => {}
        });

        if let PumpStatus::Exit(_) = status {
            tracing::info!("window closed, shutting down");
            break;
        }

        // The same request the hardware backend honours. Nested it means
        // closing the window rather than handing back a VT, but a binding that
        // works on one backend and silently does nothing on the other is worse
        // than not having it.
        if matches!(state.request.take(), Some(crate::state::Request::Quit)) {
            tracing::info!("asked to stop");
            break;
        }

        if !monitor_reported && let Some(monitor) = backend.window().current_monitor() {
            let position = monitor.position();
            tracing::info!(
                monitor = monitor.name().unwrap_or_else(|| "unknown".to_owned()),
                x = position.x,
                y = position.y,
                "nested window is on this host monitor"
            );
            monitor_reported = true;
        }

        // One clock, sampled once, before anything reads it. Two samples in a
        // frame would let two windows animate from different instants.
        state.clock.tick();
        let now = state.clock.now();

        // Scripted input, fired through the same paths a keypress and a click
        // take. A knob that bypassed them would prove the knob works.
        while triggers.last().is_some_and(|(at, _)| now >= *at) {
            if let Some((_, combo)) = triggers.pop() {
                tracing::info!(combo, "scripted trigger");
                state.trigger(&combo);
            }
        }
        while drags.last().is_some_and(|(at, _, _)| now >= *at) {
            if let Some((_, from, to)) = drags.pop() {
                tracing::info!(?from, ?to, "scripted drag");
                synth::drag(&mut state, &output, from.into(), to.into(), 12);
            }
        }
        while clicks.last().is_some_and(|(at, _)| now >= *at) {
            if let Some((_, (x, y))) = clicks.pop() {
                tracing::info!(x, y, "scripted click");
                state.trigger_click(x, y);
            }
        }

        let size = backend.window_size();
        let damage = Rectangle::from_size(size);

        // How many frames ago this buffer was last drawn, which is what lets
        // the damage tracker work out what is stale in it. Passing 0 means
        // "contents unknown", and the tracker then redraws everything every
        // frame and reports damage every frame — an idle compositor that never
        // stops rendering, which is exactly what it was doing.
        let age = backend.buffer_age().unwrap_or(0);

        // The renderer borrow must end before submit(), so rendering happens in
        // its own scope and only two flags escape.
        let (rendered, captured) = match backend.bind() {
            Err(err) => {
                tracing::warn!(?err, "could not bind the backend buffer, skipping frame");
                (false, false)
            }
            Ok((renderer, mut framebuffer)) => {
                // Every window reaches the screen through the presentation
                // transform, so a mode cannot animate differently from the
                // layout -- they are the same code path.
                let elements = render::elements(&mut state, renderer, 1.0);

                let result = damage_tracker.render_output(
                    renderer,
                    &mut framebuffer,
                    age,
                    &elements,
                    [0.05, 0.05, 0.06, 1.0],
                );
                if let Err(err) = &result {
                    tracing::warn!(?err, "render failed");
                }
                let damaged = result.as_ref().is_ok_and(|output| output.damage.is_some());

                let mut captured = false;
                let due = match capture_due {
                    Some(at) => now >= at,
                    None => settled > CAPTURE_SETTLE_FRAMES,
                };
                if result.is_ok()
                    && due
                    && capture_remaining > 0
                    && let Some(path) = capture.as_ref()
                {
                    captured = true;
                    // Numbered only when there is a sequence, so a single
                    // capture keeps the name it was given.
                    let path = if dev::capture_frames() > 1 {
                        numbered(path, capture_index)
                    } else {
                        path.clone()
                    };
                    match capture::take_frame(renderer, &framebuffer, size.w, size.h, &path) {
                        Ok(()) => tracing::debug!(path = %path.display(), "captured a frame"),
                        Err(err) => tracing::warn!(?err, "capturing a frame failed"),
                    }

                    capture_index += 1;
                    capture_remaining -= 1;
                    capture_due = if capture_remaining > 0 {
                        Some(now + capture_interval)
                    } else {
                        capture.take();
                        None
                    };
                }

                (result.is_ok() && damaged, captured)
            }
        };

        // Reading the framebuffer back invalidates the bind, so a captured
        // frame is not presented. One frame of a 60 Hz window is not visible,
        // and attempting the submit anyway costs an EGL surface reallocation
        // that fails.
        if rendered
            && !captured
            && let Err(err) = backend.submit(Some(&[damage]))
        {
            tracing::warn!(?err, "submit failed");
        }

        state.space.elements().for_each(|window| {
            window.send_frame(
                &output,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default(),
                // Throttled to the output's refresh, not to nothing: see the
                // note on `send_frames` in tty.rs. Zero here means every client
                // redraws as fast as it can for as long as it is open.
                Some(frame_interval(&output)),
                |_, _| Some(output.clone()),
            );
        });

        settled = if state.space.elements().next().is_some() {
            settled.saturating_add(1)
        } else {
            0
        };

        // Retire transforms that have landed, so a settled window costs nothing
        // to draw. Every window is visited deliberately: a short-circuiting
        // check would leave later windows transformed forever.
        for window in state.space.elements() {
            present::settle(window, now);
        }

        frames += 1;
        if now.saturating_sub(window_started) >= Duration::from_secs(2) {
            let elapsed = now.saturating_sub(window_started).as_secs_f64();
            let fps = f64::from(frames) / elapsed;
            tracing::debug!(fps = format!("{fps:.1}"), "frame pacing");
            frames = 0;
            window_started = now;
        }

        state.space.refresh();
        state.popups.cleanup();
        if let Err(err) = state.display_handle.flush_clients() {
            tracing::warn!(?err, "flushing clients failed");
        }

        // How long to wait for the next event, and the reason the drag used to
        // trail the cursor. A flat 16 ms caps the compositor at ~60 fps however
        // fast the display runs; on a 260 Hz screen the window lands four
        // frames behind the pointer. While anything is moving the wait is
        // short and `submit` above paces us against the host's vblank instead;
        // when nothing is moving there is nothing to be quick for.
        let timeout = if rendered {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(16)
        };
        if event_loop.dispatch(Some(timeout), &mut state).is_err() {
            break;
        }
    }

    Ok(())
}

/// How long one frame of an output lasts. Refresh is in millihertz.
fn frame_interval(output: &smithay::output::Output) -> std::time::Duration {
    let refresh = output
        .current_mode()
        .map(|mode| mode.refresh)
        .filter(|refresh| *refresh > 0)
        .unwrap_or(60_000);
    std::time::Duration::from_nanos(
        1_000_000_000_000_u64 / u64::try_from(refresh).unwrap_or(60_000),
    )
}
