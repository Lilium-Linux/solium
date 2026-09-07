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
            DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, DrmEventTime, DrmNode, NodeType,
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
        },
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::gles::GlesRenderer,
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
    wayland::dmabuf::DmabufFeedbackBuilder,
};

use crate::{
    render,
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

/// How long one frame of an output lasts.
///
/// The refresh rate is in millihertz, so this is a period in nanoseconds, with
/// 60 Hz as the fallback for an output that does not say.
fn frame_interval(output: &Output) -> Duration {
    let refresh = output
        .current_mode()
        .map(|mode| mode.refresh)
        .filter(|refresh| *refresh > 0)
        .unwrap_or(60_000);
    Duration::from_nanos(1_000_000_000_000_u64 / u64::try_from(refresh).unwrap_or(60_000))
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

/// The mode a connector was asked for, or the best it offers.
///
/// An exact resolution match, at the requested refresh rate when one was given
/// and at the highest available when it was not. Anything else warns and falls
/// back to the automatic choice: a configuration that asks for a mode the
/// monitor does not have should cost a line in the log, never a black screen.
fn chosen_mode(
    connector: &connector::Info,
    wanted: Option<(i32, i32, Option<i32>)>,
    name: &str,
) -> Option<DrmMode> {
    let Some((width, height, refresh)) = wanted else {
        return preferred_mode(connector);
    };

    let matching: Vec<&DrmMode> = connector
        .modes()
        .iter()
        .filter(|mode| i32::from(mode.size().0) == width && i32::from(mode.size().1) == height)
        .collect();

    let picked = match refresh {
        // Within one hertz: a 59.94 Hz mode is reported as 60 and asking for
        // 60 must find it.
        Some(refresh) => matching
            .iter()
            .find(|mode| {
                i32::try_from(mode.vrefresh())
                    .unwrap_or(0)
                    .abs_diff(refresh)
                    <= 1
            })
            .copied(),
        None => matching.iter().max_by_key(|mode| mode.vrefresh()).copied(),
    };

    match picked {
        Some(mode) => Some(*mode),
        None => {
            tracing::warn!(
                monitor = name,
                asked = format!(
                    "{width}x{height}{}",
                    refresh.map(|r| format!("@{r}")).unwrap_or_default()
                ),
                "this monitor has no such mode -- using its best instead"
            );
            preferred_mode(connector)
        }
    }
}

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

    let loop_handle = event_loop.handle();
    crate::xwayland::start(&loop_handle, &display_handle);

    let config = Scripts::config_path();
    solium.start_scripts(match Scripts::load(&config) {
        Ok(scripts) => Some(scripts),
        Err(err) => {
            tracing::error!(?err, config = %config.display(), "no scripts loaded");
            None
        }
    });

    let mut state = State {
        solium,
        session,
        renderer: None,
        screens: Vec::new(),
        input: None,
        animating: false,
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
        .insert_source(drm_events, move |event, metadata, state| match event {
            // Which CRTC flipped, and therefore which monitor. Ignored while
            // there was one screen; with two it is the whole of telling them
            // apart, and answering the wrong one's clients with this one's
            // timestamp is exactly what presentation-time exists to prevent.
            DrmEvent::VBlank(crtc) => {
                let Some(screen) = state
                    .screens
                    .iter_mut()
                    .find(|screen| screen.crtc == crtc)
                else {
                    // A flip from a CRTC we do not drive. Nothing to do with
                    // it, and nothing to be alarmed about either.
                    return;
                };
                if let Err(err) = screen.compositor.frame_submitted() {
                    tracing::warn!(?err, "the frame that just flipped was not accepted");
                }

                // The frame is on the screen, and *this* is the moment clients
                // asked about. The kernel's own flip timestamp and sequence
                // number, not ours: a number we invented here would be a guess
                // at the thing the protocol exists to stop clients guessing.
                if let Some(mut feedback) = screen.pending_feedback.take() {
                    let (time, sequence) = match metadata.as_ref().map(|it| (it.time, it.sequence)) {
                        Some((DrmEventTime::Monotonic(time), sequence)) => (time, sequence),
                        // Realtime, or no metadata at all on a driver that does
                        // not provide it. Discarded rather than answered with
                        // our own clock, because the client was told these
                        // timestamps are CLOCK_MONOTONIC and a realtime one
                        // would be off by the epoch.
                        _ => {
                            feedback.discarded();
                            screen.pending = false;
                            return;
                        }
                    };
                    // This monitor's refresh, not some other monitor's: a
                    // client on a 60 Hz panel told it has 3.8 ms to draw will
                    // miss every frame it is paced by.
                    let refresh = screen
                        .output
                        .current_mode()
                        .map(|mode| {
                            std::time::Duration::from_secs_f64(
                                1000.0 / f64::from(mode.refresh.max(1)) / 1000.0,
                            )
                        })
                        .unwrap_or_else(|| std::time::Duration::from_millis(16));
                    feedback.presented::<_, smithay::utils::Monotonic>(
                        time,
                        smithay::wayland::presentation::Refresh::fixed(refresh),
                        u64::from(sequence),
                        smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync,
                    );
                }
                // This screen's pipeline is free, and its clients may draw the
                // next frame. Only this screen's: a monitor still waiting on
                // its own flip has not freed anything.
                screen.pending = false;
                state.send_frames();
                // `owed` as well as the usual two: this screen may have been
                // skipped while it was busy, and nothing else is going to ask
                // on its behalf.
                let owed = state.screens.iter().any(|screen| screen.owed);
                if state.solium.redraw || state.animating || owed {
                    state.render();
                }
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
                // No vblank is coming while the session is away, so a frame
                // left marked in-flight would block every render on return --
                // on every screen, because every screen has its own.
                for screen in &mut state.screens {
                    screen.pending = false;
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
                for screen in &mut state.screens {
                    if let Err(err) = screen.compositor.reset_state() {
                        tracing::warn!(
                            ?err,
                            monitor = screen.output.name(),
                            "could not reset a display after resuming"
                        );
                    }
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
    //
    // The deadline is generous on purpose, and it is measured rather than
    // guessed. libinput takes between three and four and a quarter seconds to
    // report the first device on the machine this was written on; the first
    // version of this waited five, lost the race, and shut down a session that
    // was working — which reads exactly like an instant crash from the other
    // side of the screen. A watchdog that fires early is worse than none,
    // because it breaks the thing it is guarding.
    //
    // Checked repeatedly rather than once, so the wait is visible in the log
    // instead of being a silent gap before a shutdown.
    let deadline = Duration::from_secs(20);
    let interval = Duration::from_secs(2);
    let mut waited = Duration::ZERO;
    event_loop
        .handle()
        .insert_source(Timer::from_duration(interval), move |_, (), state| {
            if state.input_devices > 0 {
                return TimeoutAction::Drop;
            }
            waited += interval;
            if waited < deadline {
                tracing::warn!(
                    seconds = waited.as_secs(),
                    "still no input devices, waiting"
                );
                return TimeoutAction::ToDuration(interval);
            }
            tracing::error!(
                seconds = deadline.as_secs(),
                "no input devices -- stopping rather than holding the display \
                 with no way to escape. check that this user is on an active seat."
            );
            state.signal.stop();
            TimeoutAction::Drop
        })
        .map_err(|err| anyhow!("arming the input watchdog: {err}"))?;

    tracing::info!(
        socket = %state.solium.socket_name,
        "solium is up on the hardware -- run clients with WAYLAND_DISPLAY set to this"
    );

    // A first frame, then the vblank drives the rest.
    state.render();

    event_loop
        .run(Some(Duration::from_millis(16)), &mut state, |state| {
            // Drawn when something has changed, and not otherwise. The test
            // used to be "is any window's drawn rect non-empty", which is true
            // of every mapped window forever: a completely still screen was
            // being redrawn every tick, on a 260 Hz panel, for nothing.
            //
            // Client damage arrives as an event, so the loop is already awake
            // when it matters; the timeout only governs how long it sleeps when
            // nothing at all is happening.
            // Before the frame is decided: a drag that moved since the last
            // one is applied once, here, however many times the mouse
            // reported it. See `settle_resize`.
            state.solium.settle_resize();
            if state.solium.redraw || state.animating {
                state.render();
            }
            state.solium.space.refresh();
            // A window that appeared or went away is a different screen, whatever
            // route it took there. Smithay drops a dead client's element during
            // `refresh` and tells nobody, so without this the last frame it was
            // in can sit there until something unrelated causes damage.
            if state.solium.sync_panes() {
                state.solium.redraw = true;
            }
            state.solium.popups.cleanup();
            let _ = state.solium.display_handle.flush_clients();
        })
        .map_err(|err| anyhow!("running the event loop: {err}"))
}

/// One monitor's pipeline: its connector, its CRTC, its buffers, its flips.
///
/// There is one of these per screen and they are genuinely independent. Two
/// monitors have two refresh rates, and their page flips arrive whenever each
/// display is ready — so `pending` and the feedback callbacks belong to a
/// screen and not to the compositor. Held as one value they were: a 60 Hz
/// display would have paced a 260 Hz one, and a flip on either would have
/// answered the other's clients with the wrong timestamp.
struct Screen {
    output: Output,
    /// Which CRTC drives it, which is how a page flip is matched back to it.
    crtc: crtc::Handle,
    compositor: Compositor,
    /// A frame has been queued on this CRTC and has not reached the screen.
    ///
    /// Building another before this one flips is work that can only be thrown
    /// away, and it is how a compositor ends up rendering faster than the
    /// display can show — which costs the GPU everything and the viewer
    /// nothing.
    pending: bool,
    /// The feedback callbacks for the frame in flight on *this* screen,
    /// waiting for the page flip that will say when it was actually shown.
    pending_feedback: Option<smithay::desktop::utils::OutputPresentationFeedback>,
    /// This screen was skipped because it was busy, and still owes a frame.
    ///
    /// Necessary the moment the pipelines are separate, and easy to miss.
    /// `redraw` is one flag for the whole compositor and it is cleared by the
    /// render that consumed it — so a 260 Hz monitor drawing while a 75 Hz one
    /// is mid-flip clears the flag on the slow monitor's behalf, and the slow
    /// monitor is left showing a frame that is one change out of date until
    /// something unrelated happens to damage the screen again.
    ///
    /// This is the same shape of bug as an animation that only advances when
    /// something else asks for a frame.
    owed: bool,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen")
            .field("output", &self.output.name())
            .field("crtc", &self.crtc)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

/// Everything the hardware backend holds, plus the compositor itself.
pub(crate) struct State {
    pub(crate) solium: Solium,
    session: LibSeatSession,
    renderer: Option<GlesRenderer>,
    /// One per connected monitor, in the order the connectors were enumerated.
    screens: Vec<Screen>,
    /// The libinput context, kept so it can be suspended and resumed.
    ///
    /// A VT switch revokes every device fd. libinput has to be told, or it
    /// comes back holding fds the kernel has already taken away — which is a
    /// session with a display and no way to talk to it.
    input: Option<Libinput>,
    /// Whether any window was still moving at the last frame.
    animating: bool,
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
        let renderer = unsafe { GlesRenderer::new(context) }.context("creating the renderer")?;
        // Hardware buffer sharing, through `zwp_linux_dmabuf_v1` and not
        // through `wl_drm`.
        //
        // `bind_wl_display` used to be here, and it was worse than having
        // nothing: it advertises the legacy `wl_drm` global, GL clients take it
        // in preference to shared memory, and every buffer they sent came back
        // `NotManaged` from the import.
        //
        // The global carries *feedback*, which is the part that matters and the
        // part that was missing. Feedback names the device a client should
        // allocate on. Without it a client is handed a list of formats and left
        // to guess which GPU they belong to; kitty guessed, and died calling a
        // null entry point — twelve times in seven seconds, in the kernel log,
        // while this compositor sat there having spawned it exactly once.
        if std::env::var_os("SOLIUM_NO_DMABUF").is_some() {
            tracing::warn!("SOLIUM_NO_DMABUF is set: clients will use shared memory");
        } else {
            let formats: Vec<_> = renderer
                .egl_context()
                .dmabuf_texture_formats()
                .iter()
                .copied()
                .collect();
            // The render node, not the primary one: that is the device a client
            // can actually open and allocate against.
            let render = node
                .node_with_type(NodeType::Render)
                .and_then(Result::ok)
                .unwrap_or(node);
            match DmabufFeedbackBuilder::new(render.dev_id(), formats).build() {
                Ok(feedback) => {
                    tracing::info!(node = %render, "advertising dmabuf with feedback");
                    self.solium.dmabuf_global = Some(
                        self.solium
                            .dmabuf_state
                            .create_global_with_default_feedback::<Solium>(
                                &self.solium.display_handle,
                                &feedback,
                            ),
                    );
                }
                Err(err) => {
                    // Shared memory still works. Advertising a fast path we
                    // cannot describe is what caused the crash in the first
                    // place, so not advertising one is the safe failure.
                    tracing::error!(?err, "no dmabuf feedback: clients will use shared memory");
                }
            }
        }

        let formats = renderer.egl_context().dmabuf_render_formats().clone();
        let found = connected(&device, &self.solium.arrangement)?;
        for (connector, crtc, mode) in found {
            let name = format!(
                "{}-{}",
                connector.interface().as_str(),
                connector.interface_id()
            );
            let (width, height) = mode.size();
            tracing::info!(
                monitor = name,
                mode = format!("{width}x{height}@{:.0}", f64::from(mode.vrefresh())),
                ?crtc,
                "driving this connector"
            );

            // Each connector gets its own surface on its own CRTC. A failure
            // here loses *one* monitor rather than the session: on a two-screen
            // desk, a compositor that refuses to start because the second
            // display did something odd is worse than one that comes up on the
            // first and says why.
            let surface = match device.create_surface(crtc, mode, &[connector.handle()]) {
                Ok(surface) => surface,
                Err(err) => {
                    tracing::error!(?err, monitor = name, "no DRM surface for this connector");
                    continue;
                }
            };
            let planes = device.planes(&crtc).ok();

            let (physical_width, physical_height) = connector.size().unwrap_or((0, 0));
            let output = Output::new(
                name.clone(),
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
            // The returned `GlobalId` is a handle, not a guard: dropping it
            // does not remove the global, which is why nothing keeps it.
            let _ = output.create_global::<Solium>(&self.solium.display_handle);
            // A rotation makes the logical size portrait -- `output_geometry`
            // applies the transform to the mode -- so layouts and work areas
            // follow it without knowing about it, and the display pipeline
            // does the turning.
            let transform = self
                .solium
                .arrangement
                .transform(&name)
                .unwrap_or(Transform::Normal);
            output.change_current_state(Some(wl_mode), Some(transform), None, Some((0, 0).into()));
            output.set_preferred(wl_mode);
            // Mapped anywhere; `place_outputs` decides where, once, from the
            // configured arrangement — the same call the nested backend makes,
            // so both get the same layout from the same configuration.
            self.solium.space.map_output(&output, (0, 0));

            let compositor = match DrmCompositor::new(
                &output,
                surface,
                planes,
                GbmAllocator::new(
                    gbm.clone(),
                    GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
                ),
                // The exporter turns a rendered buffer into something the
                // display can scan out; the GBM device alone is not that.
                GbmFramebufferExporter::new(gbm.clone(), node.into()),
                COLOR_FORMATS,
                formats.clone(),
                device.cursor_size(),
                Some(gbm.clone()),
            ) {
                Ok(compositor) => compositor,
                Err(err) => {
                    tracing::error!(?err, monitor = name, "no display pipeline for this monitor");
                    self.solium.space.unmap_output(&output);
                    continue;
                }
            };

            self.screens.push(Screen {
                output,
                crtc,
                compositor,
                pending: false,
                pending_feedback: None,
                owed: false,
            });
        }

        if self.screens.is_empty() {
            return Err(anyhow!("no connected display could be driven"));
        }
        tracing::info!(monitors = self.screens.len(), "displays up");
        // Positions, then the anchored surfaces that depend on them.
        self.solium.place_outputs();

        self.renderer = Some(renderer);
        self.drm = Some(device);
        Ok(notifier)
    }

    /// Draw a frame on every screen that is ready for one.
    ///
    /// A monitor still waiting on its own page flip is skipped and the others
    /// are drawn anyway. That is the point of the pipelines being separate: a
    /// 60 Hz display must not pace a 260 Hz one, and it would if either being
    /// busy stopped the whole frame.
    fn render(&mut self) {
        if !self.active {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };

        // Read once for the whole frame, so everything animating on every
        // screen agrees about when "now" is.
        let now = self.solium.clock.now();
        // Cleared before drawing, not after: a client that commits while we
        // are rendering has damaged the *next* frame, not this one.
        self.solium.redraw = false;

        // Once per frame and not once per screen: offscreen captures, the QML
        // tick, the window list. See `render::prepare`. It also has to happen
        // before any output's buffer is bound, because it binds framebuffers
        // of its own.
        let prepared = render::prepare(&mut self.solium, renderer, 1.0);

        for index in 0..self.screens.len() {
            // Indexed rather than iterated: building a screen's elements needs
            // `&mut self.solium` as well as `&mut` that screen, and they are
            // fields of the same struct.
            let Some(screen) = self.screens.get(index) else {
                continue;
            };
            if screen.pending {
                // Remembered, because the flag that asked for this frame is
                // about to be cleared by the screens that could draw it.
                if let Some(screen) = self.screens.get_mut(index) {
                    screen.owed = true;
                }
                continue;
            }
            let output = screen.output.clone();
            let area = self
                .solium
                .space
                .output_geometry(&output)
                .unwrap_or_default();
            let Some(renderer) = self.renderer.as_mut() else {
                return;
            };
            let elements = render::elements(&mut self.solium, renderer, 1.0, &prepared, area);

            let Some(screen) = self.screens.get_mut(index) else {
                continue;
            };
            // Drawn now, whatever the outcome: an empty result means there was
            // nothing on this screen to draw, which settles the debt as surely
            // as a queued frame does.
            screen.owed = false;
            match screen.compositor.render_frame(
                renderer,
                &elements,
                [0.05, 0.05, 0.06, 1.0],
                FrameFlags::DEFAULT,
            ) {
                Ok(result) if !result.is_empty => match screen.compositor.queue_frame(()) {
                    Ok(()) => {
                        screen.pending = true;
                        // Taken now, reported at *this* screen's flip. The
                        // callbacks belong to the frame just queued here, and
                        // neither a later frame's commits nor another
                        // monitor's may be answered with this one's timestamp.
                        screen.pending_feedback = Some(self.solium.presentation_feedback(&output));
                    }
                    Err(err) => {
                        tracing::warn!(?err, monitor = output.name(), "could not queue a frame");
                    }
                },
                Ok(_) => {}
                Err(err) => tracing::warn!(?err, monitor = output.name(), "rendering failed"),
            }
        }

        self.animating = self.solium.settle(now);
    }

    /// Tell clients the frame reached the screen and they may draw the next.
    ///
    /// Sent on the page flip rather than on every render, and throttled to the
    /// output's own refresh rather than to nothing at all. `Duration::ZERO`
    /// means "draw again immediately", and sending that on every pass through
    /// the loop is an invitation a client will accept: an idle kitty sat at
    /// better than thirty percent of a core answering it. A client should be
    /// paced by the display, which is the only thing that can actually show
    /// its work.
    fn send_frames(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        // Every screen, each at its own refresh. A window on a 60 Hz monitor
        // paced by a 260 Hz one is a client asked to draw four times as often
        // as anything can show it; the other way round it misses every frame.
        // Smithay throttles per output, so a window straddling two monitors is
        // correctly paced by both rather than by whichever asked last.
        for screen in &self.screens {
            let output = &screen.output;
            let throttle = frame_interval(output);
            for window in self.solium.space.elements() {
                window.send_frame(output, now, Some(throttle), |_, _| Some(output.clone()));
            }
        }
    }
}

/// Every connected connector with a mode, each given a CRTC of its own.
///
/// A CRTC is the hardware that scans a buffer out to a connector, and there
/// are a fixed few of them — usually four. Two monitors need two, and they
/// must be *different* ones: hand the same CRTC to both connectors and the
/// second modeset takes the first's screen away, which looks like the first
/// monitor going black the moment the second is set up.
///
/// Assigned greedily, in connector order, from the CRTCs each connector's
/// encoders can actually reach. Greedy is not optimal — a card where the last
/// connector can only use a CRTC an earlier one already took would lose that
/// monitor, and a matching algorithm would not. That case needs hardware that
/// restricts routing far more than anything current does, so the cost of
/// getting it wrong is one monitor and a line in the log rather than a
/// session, and it is written down here rather than discovered.
fn connected(
    device: &DrmDevice,
    arrangement: &crate::monitor::Arrangement,
) -> Result<Vec<(connector::Info, crtc::Handle, DrmMode)>> {
    let resources = device.resource_handles().context("reading DRM resources")?;
    let mut found = Vec::new();
    let mut taken: Vec<crtc::Handle> = Vec::new();

    for handle in resources.connectors() {
        let Ok(connector) = device.get_connector(*handle, false) else {
            continue;
        };
        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        if connector.state() != connector::State::Connected {
            continue;
        }
        // Switched off in the configuration. Skipped before a CRTC is claimed,
        // so it frees one for another monitor rather than merely staying dark
        // — which is the point of turning a screen off on a card with three
        // CRTCs and four connectors.
        if !arrangement.enabled(&name) {
            tracing::info!(
                monitor = name,
                "connected, and switched off by configuration"
            );
            continue;
        }
        let Some(mode) = chosen_mode(&connector, arrangement.mode(&name), &name) else {
            // Connected and offering nothing to drive it with. Rare, and worth
            // a line: from the other side of the screen it is indistinguishable
            // from the compositor ignoring the monitor.
            tracing::warn!(monitor = name, "connected but offers no usable mode");
            continue;
        };

        let mut chosen = None;
        for encoder in connector.encoders() {
            let Ok(encoder) = device.get_encoder(*encoder) else {
                continue;
            };
            chosen = resources
                .filter_crtcs(encoder.possible_crtcs())
                .into_iter()
                .find(|crtc| !taken.contains(crtc));
            if chosen.is_some() {
                break;
            }
        }
        match chosen {
            Some(crtc) => {
                taken.push(crtc);
                found.push((connector, crtc, mode));
            }
            None => tracing::warn!(
                monitor = name,
                "no free CRTC for this connector -- it will stay dark"
            ),
        }
    }

    if found.is_empty() {
        return Err(anyhow!("no connected display with a usable mode"));
    }
    Ok(found)
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
            // The first monitor, as the region an absolute device's positions
            // are measured against. Right for a single screen and a guess with
            // several: libinput can say which output a touchscreen or tablet is
            // glued to, and reading that is what makes touch land on the right
            // monitor. Filed as #42 rather than guessed at here -- a mouse and
            // a keyboard are unaffected, because relative motion is bounded by
            // every screen instead.
            let Some(output) = state.screens.first().map(|screen| screen.output.clone()) else {
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
    let region = state
        .solium
        .space
        .output_geometry(output)
        .unwrap_or_default();
    crate::input::handle(&mut state.solium, region, event);

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
            Request::Reload => state.solium.reload(),
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
