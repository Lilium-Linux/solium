//! Running on the hardware: DRM output, libinput devices, a libseat session.
//!
//! The winit backend is for development — a window inside somebody else's
//! compositor. This is the one that makes Solium a session you can log into.
//!
//! Three pieces, and each exists to avoid needing root:
//!
//! * **libseat** takes the seat and opens the DRM and input devices on our
//!   behalf. A compositor that needs `sudo` is a compositor nobody should get
//!   used to running.
//! * **DRM** drives the display directly: connectors, modes, page flips paced
//!   by the vblank rather than by a timer.
//! * **libinput** provides real devices, and feeds the same `input::handle`
//!   seam the winit backend does — so the input profile, the bindings and the
//!   scripts do not know or care which backend is underneath.
//!
//! Switching away with Ctrl+Alt+F-something pauses the session: every device is
//! released and rendering stops until it comes back. Getting that wrong leaves
//! a black screen on return, which is the failure this file most needs to not
//! have.

use std::{
    os::fd::{AsFd, BorrowedFd},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use smithay::{
    backend::{
        allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        drm::{
            DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, DrmNode, NodeType,
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
        },
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{ImportEgl as _, gles::GlesRenderer},
        session::{Event as SessionEvent, Session as _, libseat::LibSeatSession},
        udev,
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop, LoopSignal,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device as _, Mode as DrmMode, ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::Display,
    },
    utils::{DeviceFd, Transform},
};

use crate::{
    layer, present, render,
    script::Scripts,
    state::{ClientState, Request, Solium},
};

/// What we ask the display for, in order of preference.
const COLOR_FORMATS: [smithay::backend::allocator::Fourcc; 2] = [
    smithay::backend::allocator::Fourcc::Argb8888,
    smithay::backend::allocator::Fourcc::Xrgb8888,
];

type Compositor =
    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// List the graphics devices and their outputs, and exit.
///
/// Safe to run inside an existing session: it opens the card read-only and
/// takes no DRM master, so it answers "will this work here" without taking the
/// screen away from whatever is currently drawing on it.
pub(crate) fn probe() -> Result<()> {
    let seat = std::env::var("XDG_SEAT").unwrap_or_else(|_| "seat0".to_owned());
    let primary = udev::primary_gpu(&seat)
        .context("asking udev for the primary GPU")?
        .ok_or_else(|| anyhow!("no GPU on seat {seat}"))?;
    println!("seat        {seat}");
    println!("primary gpu {}", primary.display());

    // Deliberately *not* Smithay's `DrmDevice`: constructing one asks to become
    // DRM master, and on drop tries to restore modesetting state it never owned
    // — which prints a permission error that reads like a failure and is not.
    // Enumerating connectors needs neither, so the probe stays what it claims to
    // be: a read-only question, quiet inside a running session.
    let card = Card(
        std::fs::File::open(&primary).with_context(|| format!("opening {}", primary.display()))?,
    );

    let resources = card.resource_handles().context("reading DRM resources")?;
    for handle in resources.connectors() {
        let Ok(connector) = card.get_connector(*handle, false) else {
            continue;
        };
        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        if connector.state() != connector::State::Connected {
            println!("  {name:<12} disconnected");
            continue;
        }
        let modes = connector.modes();
        let preferred = preferred_mode(&connector);
        println!(
            "  {name:<12} connected, {} modes, preferred {}",
            modes.len(),
            preferred.map_or_else(
                || "none".to_owned(),
                |mode| format!(
                    "{}x{}@{:.0}",
                    mode.size().0,
                    mode.size().1,
                    f64::from(mode.vrefresh())
                )
            )
        );
    }
    Ok(())
}

/// A DRM device opened only to be asked questions.
///
/// The `drm` traits are blanket-implemented for anything that can lend a file
/// descriptor, so this is the whole of what the probe needs.
struct Card(std::fs::File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl smithay::reexports::drm::Device for Card {}
impl smithay::reexports::drm::control::Device for Card {}

/// The best mode a connector offers.
///
/// The EDID's preferred flag names a *resolution*, and usually pairs it with a
/// pedestrian refresh rate — a 260 Hz panel reports 2560x1440@60 as preferred.
/// Taking that literally would drive a fast display slowly and make every
/// animation in the compositor look worse than it is, so the highest refresh at
/// the preferred resolution wins.
fn preferred_mode(connector: &connector::Info) -> Option<DrmMode> {
    let modes = connector.modes();
    let preferred = modes
        .iter()
        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
        .or_else(|| modes.first())?;

    modes
        .iter()
        .filter(|mode| mode.size() == preferred.size())
        .max_by_key(|mode| mode.vrefresh())
        .or(Some(preferred))
        .copied()
}

pub(crate) fn run() -> Result<()> {
    let mut event_loop: EventLoop<State> =
        EventLoop::try_new().context("creating the event loop")?;
    let display: Display<Solium> = Display::new().context("creating the wayland display")?;
    let display_handle = display.handle();

    // The session comes first: everything else is opened through it.
    let (session, notifier) = LibSeatSession::new().context("taking the seat")?;
    let seat_name = session.seat();
    tracing::info!(seat = seat_name, "session acquired");

    let mut solium = Solium::new(display_handle.clone());
    solium.socket_name = start_socket(&mut event_loop, display)?;

    let config = Scripts::config_path();
    solium.scripts = match Scripts::load(&config) {
        Ok(scripts) => Some(scripts),
        Err(err) => {
            tracing::error!(?err, config = %config.display(), "no scripts loaded");
            None
        }
    };

    let mut state = State {
        solium,
        session,
        renderer: None,
        compositor: None,
        output: None,
        input: None,
        input_devices: 0,
        drm: None,
        signal: event_loop.get_signal(),
        active: true,
    };

    let drm_events = state.open_gpu(&seat_name)?;
    state.input = Some(start_input(&mut event_loop, &state.session, &seat_name)?);

    // The vblank is what paces rendering: a frame is drawn when the last one
    // has actually reached the screen, rather than on a timer that has no idea.
    event_loop
        .handle()
        .insert_source(drm_events, move |event, _metadata, state| match event {
            DrmEvent::VBlank(_) => {
                if let Some(compositor) = state.compositor.as_mut()
                    && let Err(err) = compositor.frame_submitted()
                {
                    tracing::warn!(?err, "the frame that just flipped was not accepted");
                }
                state.render();
            }
            DrmEvent::Error(err) => tracing::error!(?err, "DRM error"),
        })
        .map_err(|err| anyhow!("watching for page flips: {err}"))?;

    // Session changes: switching away must release the devices, and switching
    // back must take them again and redraw. Skipping either half is what leaves
    // a black screen on return.
    event_loop
        .handle()
        .insert_source(notifier, move |event, (), state| match event {
            SessionEvent::PauseSession => {
                tracing::info!("session paused");
                state.active = false;
                // Every device has to be handed back, and each one has to be
                // handed back by us. libinput keeps polling fds the kernel has
                // revoked otherwise, and the DRM device keeps a master lock
                // that the compositor taking over the screen now needs.
                if let Some(input) = state.input.as_mut() {
                    input.suspend();
                }
                if let Some(drm) = state.drm.as_mut() {
                    drm.pause();
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session resumed");
                // And taken back in the same order, before anything tries to
                // use them. Skipping either half is what turns a VT switch into
                // a session with a dead screen and dead input -- and with input
                // dead, the key that would switch away again does not work
                // either, so there is no way out but the power button.
                if let Some(input) = state.input.as_mut()
                    && input.resume().is_err()
                {
                    tracing::error!("libinput did not resume: input is gone");
                }
                if let Some(drm) = state.drm.as_mut()
                    && let Err(err) = drm.activate(true)
                {
                    tracing::error!(?err, "the GPU did not come back");
                }
                if let Some(compositor) = state.compositor.as_mut()
                    && let Err(err) = compositor.reset_state()
                {
                    tracing::warn!(?err, "could not reset the display after resuming");
                }
                state.active = true;
                state.render();
            }
        })
        .map_err(|err| anyhow!("watching the session: {err}"))?;

    // A compositor with no input devices has taken the machine hostage: it
    // holds the display, and there is no key anyone can press to get it back —
    // not even the VT switch, which is itself a key. Better to give the screen
    // back and say why than to sit there looking like a crash.
    event_loop
        .handle()
        .insert_source(
            Timer::from_duration(Duration::from_secs(5)),
            |_, (), state| {
                if state.input_devices == 0 {
                    tracing::error!(
                        "no input devices after 5s -- stopping rather than holding the display \
                     with no way to escape. check that this user is on an active seat."
                    );
                    state.signal.stop();
                }
                TimeoutAction::Drop
            },
        )
        .map_err(|err| anyhow!("arming the input watchdog: {err}"))?;

    tracing::info!(
        socket = %state.solium.socket_name,
        "solium is up on the hardware -- run clients with WAYLAND_DISPLAY set to this"
    );

    // A first frame, then the vblank drives the rest.
    state.render();

    event_loop
        .run(Some(Duration::from_millis(16)), &mut state, |state| {
            // Animations keep running between input events, so a frame is
            // scheduled whenever anything is still moving.
            let now = state.solium.clock.now();
            let animating = state.solium.space.elements().any(|window| {
                state
                    .solium
                    .outer_geometry(window)
                    .is_some_and(|outer| !present::frame(window, outer, now).rect.is_empty())
            });
            if animating {
                state.render();
            }
            state.solium.space.refresh();
            state.solium.popups.cleanup();
            let _ = state.solium.display_handle.flush_clients();
        })
        .map_err(|err| anyhow!("running the event loop: {err}"))
}

/// Everything the hardware backend holds, plus the compositor itself.
struct State {
    solium: Solium,
    session: LibSeatSession,
    renderer: Option<GlesRenderer>,
    compositor: Option<Compositor>,
    output: Option<Output>,
    /// The libinput context, kept so it can be suspended and resumed.
    ///
    /// A VT switch revokes every device fd. libinput has to be told, or it
    /// comes back holding fds the kernel has already taken away — which is a
    /// session with a display and no way to talk to it.
    input: Option<Libinput>,
    /// How many input devices libinput has handed us.
    ///
    /// Zero is not a slow start, it is a session nobody can talk to — see the
    /// watchdog in `run`.
    input_devices: usize,
    /// Held for the session's lifetime: dropping it closes the device.
    drm: Option<DrmDevice>,
    signal: LoopSignal,
    active: bool,
}

impl State {
    /// Open the GPU, pick a connector, and set up its output.
    fn open_gpu(&mut self, seat: &str) -> Result<DrmDeviceNotifier> {
        let path: PathBuf = udev::primary_gpu(seat)
            .context("asking udev for the primary GPU")?
            .ok_or_else(|| anyhow!("no GPU on seat {seat}"))?;
        let node = DrmNode::from_path(&path).context("reading the DRM node")?;
        // The primary node is the one that can modeset; falling back to the
        // node we opened is right for a card that has only one.
        let node = node
            .node_with_type(NodeType::Primary)
            .and_then(Result::ok)
            .unwrap_or(node);
        tracing::info!(gpu = %path.display(), ?node, "opening the GPU");

        let fd = self
            .session
            .open(
                &path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(|err| anyhow!("opening {} through the session: {err}", path.display()))?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (mut device, notifier) =
            DrmDevice::new(fd.clone(), true).context("initialising DRM")?;
        let gbm = GbmDevice::new(fd).context("creating the GBM device")?;

        // SAFETY: the GBM device outlives the display; both are moved into the
        // compositor below.
        #[expect(unsafe_code, reason = "EGLDisplay::new is unsafe by contract")]
        let egl = unsafe { EGLDisplay::new(gbm.clone()) }.context("creating the EGL display")?;
        let context = EGLContext::new(&egl).context("creating the EGL context")?;
        #[expect(unsafe_code, reason = "GlesRenderer::new is unsafe by contract")]
        let mut renderer =
            unsafe { GlesRenderer::new(context) }.context("creating the renderer")?;
        if let Err(err) = renderer.bind_wl_display(&self.solium.display_handle) {
            // Not fatal: clients fall back to shared memory, more slowly.
            tracing::warn!(?err, "no hardware buffer sharing for clients");
        }

        let (connector, crtc, mode) = first_output(&device)?;
        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        let (width, height) = mode.size();
        tracing::info!(
            output = name,
            mode = format!("{width}x{height}@{:.0}", f64::from(mode.vrefresh())),
            "driving this connector"
        );

        let surface = device
            .create_surface(crtc, mode, &[connector.handle()])
            .context("creating the DRM surface")?;
        let planes = device.planes(&crtc).ok();

        let (physical_width, physical_height) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            name,
            PhysicalProperties {
                size: (
                    i32::try_from(physical_width).unwrap_or_default(),
                    i32::try_from(physical_height).unwrap_or_default(),
                )
                    .into(),
                subpixel: Subpixel::Unknown,
                make: "Solium".into(),
                model: "DRM".into(),
            },
        );
        let wl_mode = Mode {
            size: (i32::from(width), i32::from(height)).into(),
            refresh: i32::try_from(mode.vrefresh()).unwrap_or(60) * 1000,
        };
        let _global = output.create_global::<Solium>(&self.solium.display_handle);
        output.change_current_state(
            Some(wl_mode),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        output.set_preferred(wl_mode);
        self.solium.space.map_output(&output, (0, 0));
        layer::arrange(&output);

        let formats = renderer.egl_context().dmabuf_render_formats().clone();
        let compositor = DrmCompositor::new(
            &output,
            surface,
            planes,
            GbmAllocator::new(
                gbm.clone(),
                GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
            ),
            // The exporter turns a rendered buffer into something the display
            // can scan out; the GBM device alone is not that.
            GbmFramebufferExporter::new(gbm.clone(), node.into()),
            COLOR_FORMATS,
            formats,
            device.cursor_size(),
            Some(gbm),
        )
        .context("setting up the display pipeline")?;

        self.renderer = Some(renderer);
        self.compositor = Some(compositor);
        self.output = Some(output);
        self.drm = Some(device);
        Ok(notifier)
    }

    /// Draw a frame and queue it for the next vblank.
    fn render(&mut self) {
        if !self.active {
            return;
        }
        let (Some(renderer), Some(compositor), Some(output)) = (
            self.renderer.as_mut(),
            self.compositor.as_mut(),
            self.output.as_ref(),
        ) else {
            return;
        };

        self.solium.clock.tick();
        let now = self.solium.clock.now();

        let elements = render::elements(&mut self.solium, renderer, 1.0);
        match compositor.render_frame(
            renderer,
            &elements,
            [0.05, 0.05, 0.06, 1.0],
            FrameFlags::DEFAULT,
        ) {
            Ok(result) if !result.is_empty => {
                if let Err(err) = compositor.queue_frame(()) {
                    tracing::warn!(?err, "could not queue a frame");
                }
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(?err, "rendering failed"),
        }

        for window in self.solium.space.elements() {
            window.send_frame(
                output,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default(),
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
            present::settle(window, now);
        }
    }
}

/// The first connected connector with a mode, and a CRTC to drive it.
fn first_output(device: &DrmDevice) -> Result<(connector::Info, crtc::Handle, DrmMode)> {
    let resources = device.resource_handles().context("reading DRM resources")?;

    for handle in resources.connectors() {
        let Ok(connector) = device.get_connector(*handle, false) else {
            continue;
        };
        if connector.state() != connector::State::Connected {
            continue;
        }
        let Some(mode) = preferred_mode(&connector) else {
            continue;
        };

        // Any encoder's CRTC will do for one output; picking properly matters
        // when there are several, which is the multi-output work.
        for encoder in connector.encoders() {
            let Ok(encoder) = device.get_encoder(*encoder) else {
                continue;
            };
            if let Some(crtc) = resources.filter_crtcs(encoder.possible_crtcs()).first() {
                return Ok((connector, *crtc, mode));
            }
        }
    }
    Err(anyhow!("no connected display with a usable mode"))
}

/// Real input devices, feeding the same seam the winit backend feeds.
fn start_input(
    event_loop: &mut EventLoop<State>,
    session: &LibSeatSession,
    seat: &str,
) -> Result<Libinput> {
    let mut context = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    context
        .udev_assign_seat(seat)
        .map_err(|()| anyhow!("libinput could not take seat {seat}"))?;
    // Cloned rather than moved: the context is refcounted, and suspending and
    // resuming it across a VT switch is the caller's job, not the backend's.
    let handle = context.clone();

    event_loop
        .handle()
        .insert_source(LibinputInputBackend::new(context), |event, (), state| {
            let Some(output) = state.output.clone() else {
                return;
            };
            handle_input(state, &output, event);
        })
        .map_err(|err| anyhow!("watching input devices: {err}"))?;
    Ok(handle)
}

/// Route a libinput event through the same profile the nested backend uses.
fn handle_input(state: &mut State, output: &Output, event: InputEvent<LibinputInputBackend>) {
    if let InputEvent::DeviceAdded { device } = &event {
        state.input_devices += 1;
        tracing::info!(device = device.name(), "input device");
    }

    // The same entry point the nested backend uses: a binding, a profile or a
    // grab must not behave differently because libinput is underneath.
    crate::input::handle(&mut state.solium, output, event);

    // Things the input layer cannot do itself, because only a backend has a
    // session to do them with.
    if let Some(request) = state.solium.request.take() {
        match request {
            Request::Vt(vt) => {
                tracing::info!(vt, "switching virtual terminal");
                if let Err(err) = state.session.change_vt(vt) {
                    tracing::warn!(?err, vt, "could not switch to that terminal");
                }
            }
            Request::Quit => {
                tracing::info!("stopping: asked to by a key");
                state.signal.stop();
            }
        }
    }

    // Input changes what is on screen, and the vblank has no way to know that.
    state.render();
}

/// Bind the Wayland socket and start accepting clients.
fn start_socket(event_loop: &mut EventLoop<State>, display: Display<Solium>) -> Result<String> {
    let source = smithay::wayland::socket::ListeningSocketSource::new_auto()
        .context("binding a wayland socket")?;
    let name = source.socket_name().to_string_lossy().into_owned();

    event_loop
        .handle()
        .insert_source(source, |stream, (), state| {
            if let Err(err) = state
                .solium
                .display_handle
                .insert_client(stream, std::sync::Arc::new(ClientState::default()))
            {
                tracing::warn!(?err, "rejecting a client we could not insert");
            }
        })
        .map_err(|err| anyhow!("inserting the socket source: {err}"))?;

    event_loop
        .handle()
        .insert_source(
            smithay::reexports::calloop::generic::Generic::new(
                display,
                smithay::reexports::calloop::Interest::READ,
                smithay::reexports::calloop::Mode::Level,
            ),
            |_, display, state| {
                // SAFETY: dispatched only here, from a single-threaded loop.
                #[expect(unsafe_code, reason = "smithay requires this to be unsafe")]
                let dispatched = unsafe { display.get_mut().dispatch_clients(&mut state.solium) };
                dispatched?;
                Ok(smithay::reexports::calloop::PostAction::Continue)
            },
        )
        .map_err(|err| anyhow!("inserting the display source: {err}"))?;

    Ok(name)
}
