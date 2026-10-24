//! Client acquire transactions and the lifetime of GPU reads.
//!
//! Smithay 0.7 signals a release point when its last `Buffer` is dropped.
//! Surface state and scanout hold those buffers, but a composed GLES frame
//! may still read them after its render elements have gone. A frame keeps a
//! snapshot until its completion fence becomes readable.
use crate::state::Solium;
use smithay::{
    backend::{
        drm::DrmDeviceFd,
        renderer::{
            gles::GlesRenderer,
            sync::SyncPoint,
            utils::{
                Buffer, RendererSurfaceStateUserData, on_commit_buffer_handler,
                with_renderer_surface_state,
            },
        },
    },
    reexports::{
        calloop::{
            Interest, LoopHandle, Mode, PostAction, RegistrationToken, channel, generic::Generic,
        },
        wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_surface_v1::{
            self, WpLinuxDrmSyncobjSurfaceV1,
        },
        wayland_server::{
            Client, DisplayHandle, Resource, Weak, backend::ObjectId,
            protocol::wl_surface::WlSurface,
        },
    },
    wayland::{
        compositor::{
            self, Blocker, BlockerState, BufferAssignment, Cacheable, SurfaceAttributes,
            TraversalAction, with_states,
        },
        drm_syncobj::{
            DrmSyncPoint, DrmSyncPointBlocker, DrmSyncPointSource, DrmSyncobjCachedState,
            DrmSyncobjState, supports_syncobj_eventfd,
        },
    },
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    os::fd::OwnedFd,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug)]
pub(crate) enum Pending {
    Acquire(DrmSyncPointSource, Client, Weak<WlSurface>, Arc<AtomicBool>),
    Cancel(ObjectId),
    Release(Held),
}

#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) protocol: Option<DrmSyncobjState>,
    pub(crate) sender: Option<channel::Sender<Pending>>,
    surfaces: Vec<Weak<WlSurface>>,
    acquires: HashMap<ObjectId, Vec<Acquire>>,
    // A failed GPU wait must never return memory still in use to a client.
    stranded: Vec<Buffer>,
}

impl Drop for State {
    fn drop(&mut self) {
        if !self.stranded.is_empty() {
            // A failed completion wait is not a release, including shutdown.
            std::mem::forget(std::mem::take(&mut self.stranded));
        }
    }
}

impl State {
    pub(crate) fn enable(&mut self, display: &DisplayHandle, device: DrmDeviceFd, dmabuf: bool) {
        if !dmabuf || self.sender.is_none() || !supports_syncobj_eventfd(&device) {
            tracing::info!("explicit sync unavailable: needs dmabuf, eventfd and an event loop");
            return;
        }
        if let Some(protocol) = &mut self.protocol {
            protocol.update_device(device);
        } else {
            self.protocol = Some(DrmSyncobjState::new::<Solium>(display, device));
            tracing::info!("advertising wp_linux_drm_syncobj_manager_v1");
        }
    }

    pub(crate) fn surface(&mut self, surface: &WlSurface) {
        compositor::add_pre_commit_hook(surface, acquire);
        compositor::add_destruction_hook(surface, |state: &mut Solium, surface| {
            if let Some(sender) = &state.explicit_sync.sender {
                let _ = sender.send(Pending::Cancel(surface.id()));
            }
        });
    }

    /// Sources from every role, including subsurfaces, cursors, lock and layer
    /// surfaces. A weak surface does not keep a disconnected client alive.
    pub(crate) fn buffers(&mut self) -> Vec<Buffer> {
        if self.protocol.is_none() {
            return Vec::new();
        }
        let mut buffers = Vec::new();
        self.surfaces.retain(|weak| {
            let Ok(surface) = weak.upgrade() else {
                return false;
            };
            if with_states(&surface, |states| {
                states
                    .data_map
                    .get::<RefCell<Explicit>>()
                    .is_some_and(|e| e.borrow().active)
            }) {
                buffers.extend(
                    with_renderer_surface_state(&surface, |render| render.buffer().cloned())
                        .flatten(),
                );
            }
            true
        });
        buffers
    }

    pub(crate) fn retire(&mut self, fence: SyncPoint, buffers: Vec<Buffer>) {
        if buffers.is_empty() || fence.is_reached() {
            return;
        }
        if let Some(sender) = &self.sender {
            match sender.send(Pending::Release(Held { fence, buffers })) {
                Ok(()) => return,
                Err(std::sync::mpsc::SendError(Pending::Release(mut held))) => {
                    self.wait(held.fence.clone(), std::mem::take(&mut held.buffers));
                    return;
                }
                Err(_) => return,
            }
        }
        self.wait(fence, buffers);
    }

    fn wait(&mut self, fence: SyncPoint, buffers: Vec<Buffer>) {
        if let Err(err) = fence.wait() {
            tracing::error!(?err, "GPU completion failed; retaining client buffers");
            self.stranded.extend(buffers);
        }
    }
}

#[derive(Debug)]
struct Acquire {
    token: RegistrationToken,
    cancelled: Arc<AtomicBool>,
    client: Client,
}
#[derive(Debug)]
struct Cancellable {
    blocker: DrmSyncPointBlocker,
    cancelled: Arc<AtomicBool>,
}
impl Blocker for Cancellable {
    fn state(&self) -> BlockerState {
        if self.cancelled.load(Ordering::SeqCst) {
            BlockerState::Released
        } else {
            self.blocker.state()
        }
    }
}

fn wake(state: &mut Solium, client: &Client) {
    let display = state.display_handle.clone();
    let compositor =
        <Solium as compositor::CompositorHandler>::client_compositor_state(state, client);
    compositor.blocker_cleared(state, &display);
}

#[derive(Default)]
struct Explicit {
    active: bool,
    tracked: bool,
}

/// Smithay 0.7's destruction hook cannot reach its private queued commits.
/// Keep their releases in the same cache, until import transfers ownership to
/// the renderer's Buffer. Dropping a discarded commit then releases it too.
#[derive(Debug, Default)]
struct DiscardRelease {
    changed: bool,
    release: Option<DrmSyncPoint>,
}

impl Cacheable for DiscardRelease {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        std::mem::take(self)
    }

    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        if self.changed {
            *into = self;
        }
    }
}

impl Drop for DiscardRelease {
    fn drop(&mut self) {
        if let Some(point) = &self.release
            && let Err(err) = point.signal()
        {
            tracing::error!(?err, "signaling a discarded explicit buffer failed");
        }
    }
}

fn disconnect(client: &Client, display: &DisplayHandle, reason: impl std::fmt::Display) {
    tracing::warn!(%reason, "cannot wait for a client acquire point");
    client.kill(
        display,
        smithay::reexports::wayland_server::backend::protocol::ProtocolError {
            code: 3,
            object_id: 1,
            object_interface: "wl_display".into(),
            message: format!("cannot wait for explicit synchronization: {reason}"),
        },
    );
}
fn failed(surface: &WlSurface, display: &DisplayHandle, reason: impl std::fmt::Display) {
    if let Some(client) = surface.client() {
        disconnect(&client, display, reason);
    }
}

fn acquire(state: &mut Solium, display: &DisplayHandle, surface: &WlSurface) {
    let (new_buffer, acquire, active) = with_states(surface, |states| {
        let (changed, new_buffer) = {
            let mut attrs = states.cached_state.get::<SurfaceAttributes>();
            let buffer = &attrs.pending().buffer;
            (
                buffer.is_some(),
                matches!(buffer, Some(BufferAssignment::NewBuffer(_))),
            )
        };
        let acquire = states
            .cached_state
            .get::<DrmSyncobjCachedState>()
            .pending()
            .acquire_point
            .clone();
        if changed {
            let release = states
                .cached_state
                .get::<DrmSyncobjCachedState>()
                .pending()
                .release_point
                .clone();
            *states.cached_state.get::<DiscardRelease>().pending() = DiscardRelease {
                changed,
                release: new_buffer.then_some(release).flatten(),
            };
        }
        let active = states
            .data_map
            .get::<RefCell<Option<WpLinuxDrmSyncobjSurfaceV1>>>()
            .and_then(|s| s.borrow().clone());
        (new_buffer, acquire, active)
    });
    if new_buffer
        && acquire.is_none()
        && let Some(active) = active
    {
        // Smithay 0.7 checks one missing point, but not both missing points.
        active.post_error(
            wp_linux_drm_syncobj_surface_v1::Error::NoAcquirePoint as u32,
            "a newly attached buffer needs acquire and release points",
        );
        return;
    }
    let (true, Some(point)) = (new_buffer, acquire) else {
        return;
    };
    let Some(client) = surface.client() else {
        return;
    };
    let (blocker, source) = match point.generate_blocker() {
        Ok(pair) => pair,
        Err(err) => {
            failed(surface, display, err);
            return;
        }
    };
    let Some(sender) = &state.explicit_sync.sender else {
        failed(surface, display, "event loop unavailable");
        return;
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    if let Err(err) = sender.send(Pending::Acquire(
        source,
        client,
        surface.downgrade(),
        cancelled.clone(),
    )) {
        failed(surface, display, err);
        return;
    }
    compositor::add_blocker(surface, Cancellable { blocker, cancelled });
}

/// Run before Smithay consumes the current buffer assignments, including
/// synchronized children applied by their parent's transaction.
pub(crate) fn before_import(state: &mut Solium, surface: &WlSurface) {
    if compositor::is_sync_subsurface(surface) {
        return;
    }
    let mut reattached = Vec::new();
    compositor::with_surface_tree_upward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |surface, states, _| {
            let next = {
                let mut attrs = states.cached_state.get::<SurfaceAttributes>();
                match &attrs.current().buffer {
                    Some(BufferAssignment::NewBuffer(buffer)) => Some(Some(buffer.clone())),
                    Some(BufferAssignment::Removed) => Some(None),
                    None => None,
                }
            };
            let Some(next) = next else {
                return;
            };
            let explicit = {
                let mut ownership = states.cached_state.get::<DiscardRelease>();
                let explicit = next.is_some() && ownership.current().release.is_some();
                if explicit {
                    // The normal import directly after this hook gives the
                    // point to Smithay's Buffer, whose last GPU/scanout
                    // reference releases it.
                    ownership.current().release.take();
                }
                explicit
            };
            if !explicit {
                // An implicit replacement or detach may have superseded a
                // cached explicit commit. Smithay's empty-state merge keeps
                // those discarded points; never attach them to a new buffer.
                let mut sync = states.cached_state.get::<DrmSyncobjCachedState>();
                sync.current().acquire_point = None;
                sync.current().release_point = None;
            }
            states
                .data_map
                .insert_if_missing(|| RefCell::new(Explicit::default()));
            let mut was_explicit = false;
            if let Some(mark) = states.data_map.get::<RefCell<Explicit>>() {
                let mut mark = mark.borrow_mut();
                was_explicit = mark.active;
                mark.active = explicit;
                if explicit && !mark.tracked {
                    state.explicit_sync.surfaces.push(surface.downgrade());
                    mark.tracked = true;
                }
            }
            if (explicit || was_explicit)
                && let Some(next) = next
            {
                // A new commit owns new points even when it reattaches the same
                // wl_buffer. Smithay 0.7 otherwise preserves the old Buffer.
                // This is done after acquire readiness, never in the precommit hook.
                // The traversal already holds this surface's mutex. Read its
                // supplied state instead of reentering with_states through
                // with_renderer_surface_state.
                let same = states
                    .data_map
                    .get::<RendererSurfaceStateUserData>()
                    .and_then(|render| {
                        render
                            .lock()
                            .ok()
                            .map(|render| render.buffer().is_some_and(|old| *old == next))
                    })
                    .unwrap_or(false);
                if same {
                    states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .buffer = Some(BufferAssignment::Removed);
                    reattached.push((surface.clone(), next));
                }
            }
        },
        |_, _, _| true,
    );
    if !reattached.is_empty() {
        // Smithay's reset keeps the existing damage counter. Replacing the
        // renderer state with Default would make successive pictures share
        // a commit counter and disappear from damage/capture caches.
        on_commit_buffer_handler::<Solium>(surface);
        for (surface, buffer) in reattached {
            with_states(&surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .buffer = Some(BufferAssignment::NewBuffer(buffer));
            });
        }
    }
}

pub(crate) fn install<S: 'static>(
    handle: &LoopHandle<'static, S>,
    solium: fn(&mut S) -> &mut Solium,
) -> anyhow::Result<channel::Sender<Pending>> {
    let (sender, source) = channel::channel();
    let nested = handle.downgrade();
    handle
        .insert_source(source, move |event, _, state| {
            let Some(nested) = nested.upgrade() else {
                return;
            };
            let channel::Event::Msg(pending) = event else {
                return;
            };
            match pending {
                Pending::Cancel(id) => {
                    if let Some(tokens) = solium(state).explicit_sync.acquires.remove(&id) {
                        for wait in tokens {
                            nested.remove(wait.token);
                            wait.cancelled.store(true, Ordering::SeqCst);
                            wake(solium(state), &wait.client);
                        }
                    }
                }
                Pending::Acquire(source, client, surface, cancelled) => {
                    let Ok(surface) = surface.upgrade() else {
                        cancelled.store(true, Ordering::SeqCst);
                        wake(solium(state), &client);
                        return;
                    };
                    let id = surface.id();
                    let display = solium(state).display_handle.clone();
                    let failure_client = client.clone();
                    let waiting_client = client.clone();
                    let registration = Rc::new(Cell::new(None));
                    let fired = registration.clone();
                    let ready_id = id.clone();
                    match nested.insert_source(source, move |_, _, state| {
                        let state = solium(state);
                        if let Some(tokens) = state.explicit_sync.acquires.get_mut(&ready_id) {
                            tokens.retain(|wait| Some(wait.token) != fired.get());
                            if tokens.is_empty() {
                                state.explicit_sync.acquires.remove(&ready_id);
                            }
                        }
                        wake(state, &client);
                        Ok(())
                    }) {
                        Ok(token) => {
                            registration.set(Some(token));
                            solium(state)
                                .explicit_sync
                                .acquires
                                .entry(id)
                                .or_default()
                                .push(Acquire {
                                    token,
                                    cancelled,
                                    client: waiting_client,
                                });
                        }
                        Err(err) => {
                            tracing::error!(?err, "registering client acquire readiness failed");
                            disconnect(&failure_client, &display, err);
                        }
                    }
                }
                Pending::Release(mut held) => retire_on(
                    &nested,
                    solium(state),
                    held.fence.clone(),
                    std::mem::take(&mut held.buffers),
                ),
            }
        })
        .map_err(|err| anyhow::anyhow!("installing explicit sync event source: {err}"))?;
    Ok(sender)
}

#[derive(Debug)]
pub(crate) struct Held {
    fence: SyncPoint,
    buffers: Vec<Buffer>,
}
impl Drop for Held {
    fn drop(&mut self) {
        // Event-loop shutdown also drops sources. Keep their client storage
        // until the queued GPU reads finish, even on that path.
        if !self.buffers.is_empty() && self.fence.wait().is_err() {
            tracing::error!(
                "GPU completion failed during source teardown; retaining client storage"
            );
            std::mem::forget(std::mem::take(&mut self.buffers));
        }
    }
}

fn retire_on<S: 'static>(
    handle: &LoopHandle<'static, S>,
    state: &mut Solium,
    fence: SyncPoint,
    buffers: Vec<Buffer>,
) {
    if fence.is_reached() {
        return;
    }
    if let Some(fd) = fence.export() {
        // The fallback retains ownership even if insert_source consumes its
        // callback on failure. No release can race the failed registration.
        let held = Rc::new(RefCell::new(Some(Held {
            fence: fence.clone(),
            buffers,
        })));
        let ready = held.clone();
        let result = handle.insert_source(
            Generic::<OwnedFd>::new(fd, Interest::READ, Mode::Level),
            move |_, _, _| {
                ready.borrow_mut().take();
                Ok(PostAction::Remove)
            },
        );
        if result.is_ok() {
            return;
        }
        if let Some(mut held) = held.borrow_mut().take() {
            state
                .explicit_sync
                .wait(fence, std::mem::take(&mut held.buffers));
        }
    } else {
        state.explicit_sync.wait(fence, buffers);
    }
}

/// Before captures or frame construction; before settling can drop a departed
/// picture. Holding all source buffers is conservative and costs handle clones.
pub(crate) fn snapshot(state: &mut Solium) -> Vec<Buffer> {
    if state.explicit_sync.protocol.is_none() {
        return Vec::new();
    }
    let mut buffers = state.explicit_sync.buffers();
    for pane in state.panes.iter() {
        if let Some(left) = pane.left()
            && let crate::pane::Remains::Picture(picture) = &left.remains
        {
            buffers.extend(picture.buffers());
        }
    }
    buffers
}

/// One fence after all rendering, including offscreen work and failed output
/// passes. Exporting it lets calloop release references without a frame poll.
pub(crate) fn submitted(state: &mut Solium, renderer: &mut GlesRenderer, buffers: Vec<Buffer>) {
    if buffers.is_empty() {
        return;
    }
    let display = renderer.egl_context().display().clone();
    #[expect(unsafe_code, reason = "flushing the compositor's current GLES context")]
    let fence = renderer.with_context(|gl| {
        let fence = smithay::backend::egl::fence::EGLFence::create(&display);
        unsafe {
            gl.Flush();
        }
        fence
    });
    match fence {
        Ok(Ok(fence)) => state.explicit_sync.retire(fence.into(), buffers),
        err => {
            tracing::warn!(
                ?err,
                "could not fence client reads; finishing before release"
            );
            #[expect(
                unsafe_code,
                reason = "GPU completion fallback when fence creation failed"
            )]
            if renderer
                .with_context(|gl| unsafe {
                    gl.Finish();
                })
                .is_err()
            {
                state.explicit_sync.stranded.extend(buffers);
            }
        }
    }
}

// Smithay owns timelines and synchronization-surface requests. The manager
// adapter repairs 0.7's destroy/recreate association: insert_if_missing leaves
// the old RefCell(None) in place instead of recording the replacement object.
use smithay::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::server::{
    wp_linux_drm_syncobj_manager_v1::{self, WpLinuxDrmSyncobjManagerV1},
    wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
};
use smithay::reexports::wayland_server::{DataInit, Dispatch};
use smithay::wayland::drm_syncobj::{
    DrmSyncobjGlobalData, DrmSyncobjSurfaceData, DrmSyncobjTimelineData,
};

impl Dispatch<WpLinuxDrmSyncobjManagerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        client: &Client,
        manager: &WpLinuxDrmSyncobjManagerV1,
        request: wp_linux_drm_syncobj_manager_v1::Request,
        data: &(),
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let replacing = match &request {
            wp_linux_drm_syncobj_manager_v1::Request::GetSurface { surface, .. }
                if with_states(surface, |states| {
                    states
                        .data_map
                        .get::<RefCell<Option<WpLinuxDrmSyncobjSurfaceV1>>>()
                        .is_some_and(|object| object.borrow().is_none())
                }) =>
            {
                Some(surface.clone())
            }
            _ => None,
        };
        let replacement = replacing.as_ref().and_then(|_| {
            // The request's new child exists before DataInit initializes it.
            // Older synchronization objects already have their typed data.
            // Enumerate IDs first: backend calls inside this callback deadlock.
            let mut ids = Vec::new();
            let _ = display
                .backend_handle()
                .with_all_objects_for(client.id(), |id| ids.push(id));
            let mut uninitialized = ids.into_iter().filter_map(|id| {
                WpLinuxDrmSyncobjSurfaceV1::from_id(display, id)
                    .ok()
                    .filter(|object| object.data::<DrmSyncobjSurfaceData>().is_none())
            });
            let object = uninitialized.next()?;
            uninitialized.next().is_none().then_some(object)
        });
        <DrmSyncobjState as Dispatch<WpLinuxDrmSyncobjManagerV1, (), Self>>::request(
            state, client, manager, request, data, display, data_init,
        );
        if let Some(surface) = replacing {
            match replacement {
                Some(replacement) => with_states(&surface, |states| {
                    if let Some(object) = states
                        .data_map
                        .get::<RefCell<Option<WpLinuxDrmSyncobjSurfaceV1>>>()
                    {
                        *object.borrow_mut() = Some(replacement);
                    }
                }),
                None => disconnect(
                    client,
                    display,
                    "cannot identify replacement synchronization object",
                ),
            }
        }
    }
}
smithay::reexports::wayland_server::delegate_global_dispatch!(Solium: [WpLinuxDrmSyncobjManagerV1: DrmSyncobjGlobalData] => DrmSyncobjState);
smithay::reexports::wayland_server::delegate_dispatch!(Solium: [WpLinuxDrmSyncobjSurfaceV1: DrmSyncobjSurfaceData] => DrmSyncobjState);
smithay::reexports::wayland_server::delegate_dispatch!(Solium: [WpLinuxDrmSyncobjTimelineV1: DrmSyncobjTimelineData] => DrmSyncobjState);
