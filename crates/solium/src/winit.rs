//! Winit backend: a window on a host compositor, for development.
//!
//! Never the path to a real session — that is DRM, later. This exists so the
//! compositor can be run and tested nested, without touching the developer's
//! live session.

use std::time::Duration;

use anyhow::{Context, Result};
use smithay::{
    backend::{
        renderer::{
            Renderer as _,
            damage::OutputDamageTracker,
            element::{Id, Kind, texture::TextureRenderElement},
            gles::GlesRenderer,
        },
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
    capture, dev, render,
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

    // X11 clients, if XWayland is installed. Started before the socket source
    // so that a client launched from a script's startup has a display to find.
    let loop_handle = event_loop.handle();
    crate::xwayland::start(&loop_handle, &display_handle);

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
    // Do not wait for the host's vblank.
    //
    // Smithay already asks for this the proper way — `init_from_attributes`
    // passes `vsync: false`, which becomes an EGL swap interval of zero — and
    // the NVIDIA driver ignores it. Its own default wins unless this variable
    // says otherwise, so the request has to be made twice, in two languages.
    //
    // It has to be off. A nested compositor that blocks in `eglSwapBuffers`
    // waiting for a host that has stopped servicing its surface does not
    // stutter, it stops: the whole event loop is in that call, so no client is
    // answered, no input is read, and nothing in the log says why.
    //
    // The way to hit it is to let the machine go to sleep. The host stops
    // compositing, the vblank being waited for never arrives, and the session
    // is dead when you come back to it — which is how this was found, and it is
    // worth naming because it is not a thing anyone thinks to test. Backtrace
    // from a real one:
    //
    //     WlEglSurface::swap_buffers
    //     EGLSurface::swap_buffers
    //     WinitGraphicsBackend::submit
    //     solium::winit::run
    //
    // The host composites this window anyway, so waiting for its refresh buys
    // nothing even when it works. Set here rather than in a script so that
    // running the binary directly cannot hit it, and only if it is unset, so
    // anyone who wants the other behaviour can still ask for it.
    if std::env::var_os("__GL_SYNC_TO_VBLANK").is_none() {
        // SAFETY: single-threaded at this point -- this runs before the event
        // loop, the renderer, and any thread that could read the environment.
        #[expect(
            unsafe_code,
            reason = "setting an environment variable before EGL reads it"
        )]
        unsafe {
            std::env::set_var("__GL_SYNC_TO_VBLANK", "0");
        }
    }

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
    // One window, and as many monitors inside it as asked for. Side by side,
    // sharing the window's width.
    //
    // This exists because nested and hardware are different compositors, and
    // that has already cost this project a cursor that was invisible for its
    // entire life. Multi-monitor is the single-output assumption removed from a
    // dozen places, and every one of them would otherwise be developed against
    // a backend where the assumption is still true and then discovered on a
    // TTY, where nothing can be read and every attempt costs a session.
    //
    // The picture is the honest one: each of these gets its own layer map, its
    // own work area, its own elements built at its own origin. What it cannot
    // simulate is a second *pipeline* — one refresh rate, one page flip, one
    // buffer — which is exactly the part `tty.rs` owns.
    let count = dev::outputs();
    // The window's pixels, split between the monitors. Each one's *logical*
    // size is this divided by its own scale, which is what `output_geometry`
    // works out and what `place_outputs` lays out.
    let width = (size.w / i32::try_from(count).unwrap_or(1)).max(1);
    let mut outputs = Vec::new();
    for index in 0..count {
        let mode = Mode {
            size: (width, size.h).into(),
            refresh,
        };
        let output = Output::new(
            if count == 1 {
                "winit".to_owned()
            } else {
                format!("winit-{}", index + 1)
            },
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Solium".into(),
                model: "Winit".into(),
            },
        );
        // Kept, unlike before: a `GlobalId` is a handle and not a guard, so
        // taking a nested monitor away needs this to remove it with. See
        // `SOLIUM_OUTPUTS_AT`.
        let global = output.create_global::<Solium>(&display_handle);
        output.change_current_state(
            Some(mode),
            Some(Transform::Flipped180),
            None,
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        // Mapped anywhere; `place_outputs` decides where. Doing it in one place
        // means the nested backend and the hardware get the same arrangement
        // from the same configuration.
        state.space.map_output(&output, (0, 0));
        outputs.push((output, global));
    }
    state.place_outputs();
    if count > 1 {
        tracing::info!(count, width, "nested with more than one monitor");
    }
    // The output every device's absolute positions are measured against: the
    // window is one surface, so a position in it spans all of them.
    let output = outputs
        .first()
        .map(|(output, _)| output.clone())
        .ok_or_else(|| anyhow::anyhow!("no outputs: SOLIUM_OUTPUTS must be at least 1"))?;

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

    // The screens exist and the scripts have loaded: whichever came second,
    // this is the first moment a script can be told where the monitors are.
    state.monitors_ready();

    // Damage is tracked against the window. With one monitor the window *is*
    // the output, so the output's own tracker is right; with several the window
    // is the desk and each monitor is a texture on it, so it is built from the
    // window's size instead.
    let mut damage_tracker = if outputs.len() > 1 {
        OutputDamageTracker::new(size, 1.0, Transform::Flipped180)
    } else {
        OutputDamageTracker::from_output(&output)
    };
    // Where each monitor sits in the global space. Refreshed every frame rather
    // than held, because `super+shift+r` can rearrange them.
    let mut screens: Vec<Rectangle<i32, smithay::utils::Logical>> = Vec::new();
    let mut monitors = crate::offscreen::Screens::new();

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
    let mut screen_changes = dev::outputs_at();
    let mut loadings = dev::loading_at();
    // Reversed so `last` is the *earliest*, which is what the `while ... pop`
    // below wants. `drags` was the one list that missed this, so with more than
    // one drag none fired until the latest was due and then they all fired at
    // once, newest first. A single drag is the same list either way, which is
    // why it went unnoticed through a dozen tests.
    triggers.reverse();
    clicks.reverse();
    loadings.reverse();
    drags.reverse();
    screen_changes.reverse();

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

    let mut resized = false;
    loop {
        let status = winit.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                // Every monitor, not just the first. Resizing only `output`
                // gave the left-hand screen the whole window's width, so it
                // then covered the right-hand one — and a pointer over the
                // second monitor was answered with the first, which is a
                // window opening on the screen you are not looking at.
                let count = i32::try_from(outputs.len()).unwrap_or(1).max(1);
                let width = (size.w / count).max(1);
                for (monitor, _) in &outputs {
                    monitor.change_current_state(
                        Some(Mode {
                            size: (width, size.h).into(),
                            refresh: 60_000,
                        }),
                        None,
                        None,
                        None,
                    );
                }
                // Their positions depend on their widths, so the arrangement
                // is decided again rather than kept.
                resized = true;
            }
            WinitEvent::Input(event) => {
                // The union, not one monitor: the host reports a position
                // within the *window*, and the window is every monitor at
                // once. Measuring against `outputs[0]` would squeeze the whole
                // window into the left-hand screen.
                let region = crate::monitor::union(&state.space)
                    .unwrap_or_else(|| Rectangle::from_size(size.to_logical(1)));
                crate::input::handle(&mut state, region, event);
            }
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
        if matches!(state.request, Some(crate::state::Request::Reload)) {
            state.request = None;
            state.reload();
        }
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

        if resized {
            resized = false;
            state.place_outputs();
        }

        screens.clear();
        screens.extend(
            state
                .space
                .outputs()
                .filter_map(|output| state.space.output_geometry(output)),
        );

        // Read once for the whole iteration, so everything animating in this
        // frame agrees about when "now" is.
        let now = state.clock.now();

        // Scripted input, fired through the same paths a keypress and a click
        // take. A knob that bypassed them would prove the knob works.
        while triggers.last().is_some_and(|(at, _)| now >= *at) {
            if let Some((_, combo)) = triggers.pop() {
                tracing::info!(combo, "scripted trigger");
                state.trigger(&combo);
            }
        }
        // A nested monitor going away and coming back. The DRM half of hotplug
        // needs a cable; this half -- what a layout does with a window whose
        // monitor has gone -- is identical nested, and it is the half that has
        // broken twice.
        while screen_changes.last().is_some_and(|(at, _)| now >= *at) {
            if let Some((_, wanted)) = screen_changes.pop() {
                tracing::info!(wanted, have = outputs.len(), "scripted monitor change");
                while outputs.len() > wanted {
                    let Some((output, global)) = outputs.pop() else {
                        break;
                    };
                    tracing::info!(monitor = output.name(), "monitor gone");
                    state.display_handle.remove_global::<Solium>(global);
                    crate::layer::close_all(&output);
                    state.space.unmap_output(&output);
                }
                while outputs.len() < wanted {
                    let index = outputs.len();
                    let output = Output::new(
                        format!("winit-{}", index + 1),
                        PhysicalProperties {
                            size: (0, 0).into(),
                            subpixel: Subpixel::Unknown,
                            make: "Solium".into(),
                            model: "Winit".into(),
                        },
                    );
                    let global = output.create_global::<Solium>(&display_handle);
                    let mode = Mode {
                        size: (width, size.h).into(),
                        refresh,
                    };
                    output.change_current_state(
                        Some(mode),
                        Some(Transform::Flipped180),
                        None,
                        Some((0, 0).into()),
                    );
                    output.set_preferred(mode);
                    state.space.map_output(&output, (0, 0));
                    tracing::info!(monitor = output.name(), "monitor arrived");
                    outputs.push((output, global));
                }
                // The same call the hardware backend makes, which is the whole
                // point of this knob existing.
                state.settle_monitors();
            }
        }

        while drags.last().is_some_and(|(at, _, _)| now >= *at) {
            if let Some((_, from, to)) = drags.pop() {
                tracing::info!(?from, ?to, "scripted drag");
                let region = crate::monitor::union(&state.space)
                    .unwrap_or_else(|| Rectangle::from_size(size.to_logical(1)));
                synth::drag(&mut state, region, from.into(), to.into(), 12);
            }
        }
        while loadings.last().is_some_and(|(at, _)| now >= *at) {
            if let Some((_, program)) = loadings.pop() {
                tracing::info!(program, "scripted loading window");
                state.begin_loading(&program, None);
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
        //
        // Zero while locked, which asks for the whole screen every time. A
        // damage-tracked frame only redraws what changed and trusts the rest
        // of the buffer to still hold the previous frame -- and the buffers in
        // a swapchain outlive the lock, so one of them holds a picture of the
        // desktop from before it. Getting the age wrong by one anywhere in the
        // loop would put that picture back on screen through the gaps in the
        // damage, which is the one thing a lock screen may never do. A locked
        // screen is static, so the whole cost of this is a full redraw on the
        // handful of frames a lock screen ever draws.
        let age = if state.lock.is_some() {
            0
        } else {
            backend.buffer_age().unwrap_or(0)
        };

        // Before the frame is decided: a drag that moved since the last one is
        // applied once, here, however many times the mouse reported it. See
        // `settle_resize`.
        state.settle_resize();
        // And a Wayland client waiting on an X11 client's clipboard. Here
        // because this is where a loop handle exists; see `settle_selection`.
        crate::xwayland::settle_selection(&mut state, &loop_handle);

        // A capture that is due needs a frame to be captured *from*. Asked for
        // rather than assumed: with drawing gated on damage a still screen
        // draws nothing, and a capture of a still screen is exactly what most
        // of these tests want to look at.
        if capture_due.is_some_and(|at| now >= at) && capture_remaining > 0 {
            state.redraw = true;
        }

        // Drawn when something has changed, and not otherwise -- the same test
        // the hardware backend makes, and deliberately so. This loop used to
        // draw every iteration, which meant every animation was developed and
        // verified against a compositor that redrew whatever happened. A
        // missing damage signal is invisible here and obvious on the hardware,
        // which is the worst possible way round.
        let wanted = state.redraw || state.animating;
        // Cleared before drawing, not after: a client that commits while we are
        // rendering has damaged the *next* frame, not this one.
        state.redraw = false;

        // The renderer borrow must end before submit(), so rendering happens in
        // its own scope and only two flags escape.
        // Before the output buffer is bound: this pass binds framebuffers of
        // its own, and doing that underneath a bound output redirects the
        // whole frame into a texture. See `render::Prepared`.
        let prepared = if wanted {
            render::prepare(&mut state, backend.renderer())
        } else {
            render::Prepared::default()
        };

        // Before the output buffer is bound, alongside `prepare` and for the
        // same reason: a capture binds a framebuffer of its own, and doing
        // that underneath a bound output redirects the whole frame into it.
        crate::screencopy::settle(&mut state, backend.renderer(), &prepared);

        // Not `match backend.bind() { _ if !wanted => ... }`: the scrutinee runs
        // before the guard, so that acquired a buffer on every idle frame and
        // threw it away without ever submitting it. The host's EGL surface ends
        // up in a state it complains about at sixty errors a second.
        let (rendered, captured) = if !wanted {
            (false, false)
        } else {
            match backend.bind() {
                Err(err) => {
                    tracing::warn!(?err, "could not bind the backend buffer, skipping frame");
                    (false, false)
                }
                Ok((renderer, mut framebuffer)) => {
                    // Every window reaches the screen through the presentation
                    // transform, so a mode cannot animate differently from the
                    // layout -- they are the same code path.
                    //
                    // One monitor takes the direct path, which is every
                    // ordinary nested run: elements straight into the window's
                    // buffer, damage tracked per element, nothing extra. More
                    // than one and each is drawn into a texture of its own
                    // first, because that is what having its own buffer means.
                    let elements = if screens.len() > 1 {
                        let mut whole = Vec::new();
                        // Each monitor takes its share of the window's
                        // *pixels*, and its logical size is that share divided
                        // by its own scale. So a 2x monitor's texture comes
                        // back twice as large in logical terms and exactly the
                        // share wide in pixels, and goes into the window one
                        // to one — which is the point: you are looking at the
                        // pixels that monitor would scan out, not a picture of
                        // them.
                        let mut at = 0.0_f64;
                        for (index, screen) in screens.iter().enumerate() {
                            let scale = state
                                .output_for(*screen)
                                .map_or(1.0, |output| output.current_scale().fractional_scale());
                            let Some((texture, pixels)) = monitors
                                .draw(&mut state, renderer, &prepared, index, *screen, scale)
                            else {
                                continue;
                            };
                            // `src` must be given whenever `size` is: Smithay
                            // defaults it to the drawn size, which crops the
                            // texture to its top-left corner instead of
                            // scaling it. Same trap `Decoration::frame`
                            // records, and it cost the same half hour again.
                            let source = Rectangle::from_size(
                                (f64::from(pixels.w), f64::from(pixels.h)).into(),
                            );
                            whole.push(render::Element::Screen(
                                TextureRenderElement::from_static_texture(
                                    Id::new(),
                                    renderer.context_id(),
                                    (at, 0.0),
                                    texture,
                                    1,
                                    Transform::Normal,
                                    None,
                                    Some(source),
                                    Some((pixels.w, pixels.h).into()),
                                    None,
                                    Kind::Unspecified,
                                ),
                            ));
                            at += f64::from(pixels.w);
                        }
                        whole
                    } else {
                        let screen = screens.first().copied().unwrap_or_default();
                        let scale = state
                            .output_for(screen)
                            .map_or(1.0, |output| output.current_scale().fractional_scale());
                        render::elements(
                            &mut state,
                            renderer,
                            &prepared,
                            render::Picture::screen(screen, scale),
                        )
                    };

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
            }
        };

        // Reading the framebuffer back invalidates the bind, so a captured
        // frame is not presented. One frame of a 60 Hz window is not visible,
        // and attempting the submit anyway costs an EGL surface reallocation
        // that fails.
        // Presentation feedback, nested.
        //
        // A compositor inside another compositor cannot know when the host
        // actually put the frame on a screen — there is no page flip here to
        // ask. So the callbacks are answered with the moment the frame was
        // handed over and *without* the Vsync flag, which is the protocol's way
        // of saying this timestamp is when it was submitted rather than when it
        // was shown. Discarding them instead would be worse: a client would
        // wait for an answer that never comes.
        if rendered
            && let Some(mut feedback) = state
                .space
                .outputs()
                .next()
                .cloned()
                .map(|output| state.presentation_feedback(&output))
        {
            // CLOCK_MONOTONIC, because that is the clock id the clients were
            // told. Not `SystemTime`: that is the realtime clock and would be
            // off by the epoch, which is the one way to make this worse than
            // saying nothing.
            let refresh = std::time::Duration::from_micros(16_667);
            feedback.presented(
                smithay::utils::Clock::<smithay::utils::Monotonic>::new().now(),
                smithay::wayland::presentation::Refresh::variable(refresh),
                0,
                smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::empty(),
            );
        }

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
        // to draw, and learn whether anything still needs the next frame.
        state.settle(now);

        frames += 1;
        if now.saturating_sub(window_started) >= Duration::from_secs(2) {
            let elapsed = now.saturating_sub(window_started).as_secs_f64();
            let fps = f64::from(frames) / elapsed;
            tracing::debug!(fps = format!("{fps:.1}"), "frame pacing");
            frames = 0;
            window_started = now;
        }

        state.space.refresh();
        // A window that appeared or went away is a different screen, whatever
        // route it took there. Smithay drops a dead client's element during
        // `refresh` and tells nobody, so without this the last frame it was
        // in can sit there until something unrelated causes damage.
        if state.sync_panes() {
            state.redraw = true;
        }
        state.popups.cleanup();
        // Who has been idle long enough to be told about it. Once a frame
        // rather than on a timer: both loops wake at least every 16 ms, and
        // a notification that arrives up to one frame late is a notification
        // about somebody having left the room.
        crate::idle::settle(&mut state);
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
