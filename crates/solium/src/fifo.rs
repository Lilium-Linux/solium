//! `wp_fifo_v1` and `wp_commit_timing_v1` (#76): clients pacing themselves by
//! the screen.
//!
//! Frame callbacks tell a client when it may draw. These two let it say when
//! what it drew may be *shown*, which is how a Vulkan swapchain in FIFO mode
//! and a video player that knows when each frame is due want to work:
//!
//! - **fifo**: a commit that waits on a barrier is held until the content that
//!   set the barrier has been on the screen for a refresh. A client can queue
//!   frames as fast as it likes, and the screen still shows each of them,
//!   one a refresh, instead of the newest replacing the rest unseen.
//! - **commit timing**: a commit stamped with a time is held until the frame
//!   the compositor draws next would reach the screen no earlier than it.
//!
//! The two are one feature, and both are cleared at the same place: where a
//! monitor's refresh tells its surfaces they may draw again,
//! `Solium::send_frames_on`, so whichever surfaces a refresh paces, it also
//! clears. That is both backends: the hardware's vblank and the passes that
//! queue nothing, and the nested backend's every frame.
//!
//! ## Only what the frame showed
//!
//! A refresh clears a barrier only if the commit that set it was applied
//! before the frame now on the screen was drawn. On the hardware a frame is
//! drawn a refresh before it flips, and a commit can land in between: its
//! barrier cleared at that flip would let the next commit replace it before
//! any frame showed it, the dropped frame fifo exists to prevent. So each
//! commit is stamped as it is applied ([`applied`]), each monitor as a pass
//! starts drawing it ([`latch`]), and a refresh compares the two.
//!
//! A timed commit is let go when the earliest a frame drawn from now can
//! reach the screen is no earlier than its time: the monitor's next vblank,
//! on the phase of its last flip ([`flipped`], `cadence::next_vblank`). At a
//! flip that is the next refresh exactly; between flips it is still never
//! early. Nested, where no vblank is known, it is now, a host frame late at
//! worst.
//!
//! ## What no refresh reaches
//!
//! A surface no lit monitor refreshes would otherwise be held for ever: a
//! window only a screen that is off shows, one the configuration has hidden, a
//! commit timed for later on a still screen that draws nothing. So every
//! commit that will be held is noted as it arrives, in [`noted`], and
//! [`settle`], once a loop iteration, lets it go when it is due:
//!
//! - a timed commit at its time, whether or not anything is drawn then;
//! - a fifo wait that nothing has cleared after `idle.off_frame_interval`,
//!   the same cadence a surface no lit screen shows is told it may draw at.
//!   0 holds it, as it stops those frame callbacks. The protocol allows both:
//!   a compositor may ignore the constraint for a surface it is not updating,
//!   and clients must not rely on it to be throttled there.
//!
//! The dark pass, `Solium::send_dark_frames`, does not clear: it runs every
//! loop iteration, and a fifo client woken by its own commit would spin the
//! loop as fast as it could commit. Smithay throttles that pass's frame
//! callbacks; [`settle`] is the same throttle for barriers.

use std::{cell::Cell, time::Duration};

use smithay::{
    desktop::{LayerSurface, Window},
    output::Output,
    reexports::wayland_server::{
        Client, DisplayHandle, Resource as _, Weak, protocol::wl_surface::WlSurface,
    },
    utils::{Monotonic, Time},
    wayland::{
        commit_timing::{
            CommitTimerBarrierStateUserData, CommitTimerStateUserData, CommitTimingManagerState,
        },
        compositor::{
            self, Barrier, CompositorHandler as _, SurfaceData, TraversalAction,
            is_sync_subsurface, with_states, with_surface_tree_downward,
        },
        fifo::{FifoBarrierCachedState, FifoCachedState, FifoManagerState},
        seat::WaylandFocus as _,
    },
};

use crate::state::Solium;

/// The two globals, and the commits they hold that no refresh may reach.
#[derive(Debug)]
pub(crate) struct State {
    _fifo: FifoManagerState,
    _timing: CommitTimingManagerState,
    /// See [`noted`]. Short: a fifo client at a monitor's rate adds one a
    /// frame, and the next [`settle`] drops it once a refresh cleared it.
    held: Vec<Held>,
}

/// A commit that will be held, and when [`settle`] lets it go.
#[derive(Debug)]
struct Held {
    surface: Weak<WlSurface>,
    due: Duration,
    /// The fifo barrier it waits on. None for a timed commit, which smithay
    /// keeps on the surface by its time: `signal_until` finds it there.
    barrier: Option<Barrier>,
}

impl State {
    /// Registers `wp_fifo_manager_v1` and `wp_commit_timing_manager_v1`.
    ///
    /// Every client may use them. Either one only ever holds back the client's
    /// own commits, never anyone else's, and never for longer than [`settle`]
    /// allows.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        Self {
            _fifo: FifoManagerState::new::<Solium>(display),
            _timing: CommitTimingManagerState::new::<Solium>(display),
            held: Vec::new(),
        }
    }
}

smithay::delegate_fifo!(Solium);
smithay::delegate_commit_timing!(Solium);

/// Watch every commit of `surface`, from its creation. Before smithay's own
/// hooks, which are added when the client asks for a fifo or a timer: so the
/// barrier a commit is about to wait on, and its timestamp, are still where
/// the client put them.
pub(crate) fn surface(surface: &WlSurface) {
    compositor::add_pre_commit_hook::<Solium, _>(surface, |state, _display, surface| {
        noted(state, surface, crate::pacing::monotonic_now());
    });
}

/// `surface` is committing at `now`: note what the commit will be held on.
fn noted(state: &mut Solium, surface: &WlSurface, now: Duration) {
    let (barrier, timestamp) = with_states(surface, |states| {
        let fifo = *states.cached_state.get::<FifoCachedState>().pending();
        let barrier = if fifo.wait_barrier {
            states
                .cached_state
                .get::<FifoBarrierCachedState>()
                .pending()
                .barrier
                .clone()
        } else {
            None
        };
        let timestamp = states
            .data_map
            .get::<CommitTimerStateUserData>()
            .and_then(|timer| timer.borrow().timestamp);
        (barrier, timestamp)
    });
    // Held only on a barrier still up, and never as a synchronised
    // subsurface, which the protocol says ignores it: as smithay decides.
    let barrier = barrier.filter(|barrier| !barrier.is_signaled() && !is_sync_subsurface(surface));
    let held = &mut state.client_pacing.held;
    if let Some(barrier) = barrier {
        let patience = state.idle.settings().off_frame_interval;
        if !patience.is_zero() {
            held.retain(|it| it.barrier.as_ref().is_none_or(|it| !it.is_signaled()));
            held.push(Held {
                surface: surface.downgrade(),
                due: now + patience,
                barrier: Some(barrier),
            });
        }
    }
    if let Some(timestamp) = timestamp {
        held.push(Held {
            surface: surface.downgrade(),
            due: Time::<Monotonic>::from(timestamp).into(),
            barrier: None,
        });
    }
}

/// When a surface's last commit was applied, on the monotonic clock.
struct Applied(Cell<Duration>);

/// When a pass last started drawing a monitor.
struct Latched(Cell<Duration>);

/// When a monitor's last frame flipped, on the hardware.
struct Flipped(Cell<Duration>);

/// `surface`'s commit is being applied now. From `Solium::commit`, which
/// smithay calls as each commit is applied, held or not.
pub(crate) fn applied(surface: &WlSurface) {
    let now = crate::pacing::monotonic_now();
    with_states(surface, |states| {
        states
            .data_map
            .get_or_insert(|| Applied(Cell::new(now)))
            .0
            .set(now);
    });
}

/// A pass starts drawing `output` at `at`: every commit applied before then
/// is in the frame it draws. Whether or not it finds anything new to queue,
/// since a commit with nothing new to show is shown already.
pub(crate) fn latch(output: &Output, at: Duration) {
    let latched = output.user_data().get_or_insert(|| Latched(Cell::new(at)));
    latched.0.set(at);
}

/// `output`'s frame flipped at `at`: the phase of its vblanks. From the
/// hardware's vblank; nested has none.
pub(crate) fn flipped(output: &Output, at: Duration) {
    let flipped = output.user_data().get_or_insert(|| Flipped(Cell::new(at)));
    flipped.0.set(at);
}

/// The earliest a frame of `output`, which refreshes every `interval`, drawn
/// from `now` can be on the screen. See the module's documentation.
pub(crate) fn shown(output: &Output, now: Duration, interval: Duration) -> Duration {
    match output.user_data().get::<Flipped>() {
        Some(flipped) => crate::cadence::next_vblank(Some(flipped.0.get()), now, interval),
        None => now,
    }
}

/// When a pass last started drawing `output`. Never drawn is before every
/// commit, so its refresh clears no barrier.
pub(crate) fn latched(output: &Output) -> Duration {
    output
        .user_data()
        .get::<Latched>()
        .map_or(Duration::ZERO, |latched| latched.0.get())
}

/// A refresh is paced to a surface: clear its fifo barrier if the frame it
/// shows, drawn at `latched`, has the commit that set it, and every commit
/// timed for no later than `shown`, the earliest a frame drawn next can be on
/// the screen. `latched` of `None` is a surface no monitor shows, cleared
/// whatever was drawn. Whether anything was cleared, for whoever wakes its
/// client.
pub(crate) fn refreshed(states: &SurfaceData, latched: Option<Duration>, shown: Duration) -> bool {
    let mut cleared = false;
    let in_frame = latched.is_none_or(|latched| {
        states
            .data_map
            .get::<Applied>()
            .is_some_and(|applied| applied.0.get() <= latched)
    });
    let barrier = in_frame
        .then(|| {
            states
                .cached_state
                .get::<FifoBarrierCachedState>()
                .current()
                .barrier
                .take()
        })
        .flatten();
    if let Some(barrier) = barrier {
        cleared |= !barrier.is_signaled();
        barrier.signal();
    }
    if let Some(timers) = states.data_map.get::<CommitTimerBarrierStateUserData>()
        && let Ok(mut timers) = timers.lock()
    {
        cleared |= timers.signal_until(Time::<Monotonic>::from(shown));
    }
    cleared
}

/// [`refreshed`] for every surface of `window`: subsurfaces and popups too.
pub(crate) fn refresh_window(
    window: &Window,
    latched: Option<Duration>,
    shown: Duration,
) -> Option<Client> {
    let mut cleared = false;
    window.with_surfaces(|_, states| cleared |= refreshed(states, latched, shown));
    cleared
        .then(|| window.wl_surface().and_then(|surface| surface.client()))
        .flatten()
}

/// [`refreshed`] for every surface of `layer`, popups too.
pub(crate) fn refresh_layer(
    layer: &LayerSurface,
    latched: Option<Duration>,
    shown: Duration,
) -> Option<Client> {
    let mut cleared = false;
    layer.with_surfaces(|_, states| cleared |= refreshed(states, latched, shown));
    cleared.then(|| layer.wl_surface().client()).flatten()
}

/// [`refreshed`] for `surface` and its subsurfaces.
pub(crate) fn refresh_tree(
    surface: &WlSurface,
    latched: Option<Duration>,
    shown: Duration,
) -> Option<Client> {
    let mut cleared = false;
    with_surface_tree_downward(
        surface,
        (),
        |_, _, ()| TraversalAction::DoChildren(()),
        |_, states, ()| cleared |= refreshed(states, latched, shown),
        |_, _, ()| true,
    );
    cleared.then(|| surface.client()).flatten()
}

/// Apply whatever `clients` have ready now that something they waited on has
/// cleared. Each client once: one call applies all its ready commits.
pub(crate) fn wake(state: &mut Solium, clients: Vec<Client>) {
    let display = state.display_handle.clone();
    let mut woken: Vec<Client> = Vec::new();
    for client in clients {
        if woken.contains(&client) {
            continue;
        }
        state
            .client_compositor_state(&client)
            .blocker_cleared(state, &display);
        woken.push(client);
    }
}

/// Once a loop iteration: let go of every held commit that is due. See "What
/// no refresh reaches" above.
pub(crate) fn settle(state: &mut Solium) {
    settle_at(state, crate::pacing::monotonic_now());
}

/// [`settle`] at `now`, which a test can move.
pub(crate) fn settle_at(state: &mut Solium, now: Duration) {
    if state.client_pacing.held.is_empty() {
        return;
    }
    let mut clients = Vec::new();
    state.client_pacing.held.retain(|held| {
        let Ok(surface) = held.surface.upgrade() else {
            return false;
        };
        if held.barrier.as_ref().is_some_and(Barrier::is_signaled) {
            return false;
        }
        if now < held.due {
            return true;
        }
        let cleared = match &held.barrier {
            Some(barrier) => {
                barrier.signal();
                true
            }
            None => with_states(&surface, |states| {
                states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .and_then(|timers| timers.lock().ok())
                    .is_some_and(|mut timers| timers.signal_until(Time::<Monotonic>::from(now)))
            }),
        };
        if cleared && let Some(client) = surface.client() {
            clients.push(client);
        }
        false
    });
    wake(state, clients);
}
