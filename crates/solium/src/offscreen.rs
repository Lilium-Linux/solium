//! Rendering a window to a texture, so it can be deformed as one thing.
//!
//! A warp needs a single texture. Take the client's surface directly and two
//! things go wrong: the frame the compositor drew stays a flat rectangle
//! beside a tilted window, and a window with subsurfaces or a popup comes
//! apart along its own seams, each piece deformed about its own centre.
//!
//! So the window is drawn once, flat, at its real size, into a texture of its
//! own — frame included — and that texture is what gets bent. It costs a pass
//! and a texture per deformed window, which is why only deformed windows pay
//! it.
//!
//! The pass is per frame and the texture is not. A window's size does not
//! change because it is being warped — `present.rs`'s first rule — so the
//! texture is a target from the renderer's pool (`pool.rs`), made once with
//! its framebuffer object, kept on the pane (`keyed::Captures`) and drawn into
//! again on every frame of the animation:
//! `keyed::tests::a_genie_costs_one_texture_and_not_one_a_frame`.
//!
//! **Built first, drawn after.** `render::prepare` builds every capture's
//! element list first ([`pane_job`], [`over_job`]), which can run Qt, and
//! then [`draw`] binds a 1x1 carrier once and draws each capture into its own
//! target, a frame each, which must not run Qt.
//! `render::tests::a_pane_neither_warped_nor_styled_wants_no_capture`.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame as _, ImportAll, Offscreen, Renderer,
            element::{
                AsRenderElements, Element as _, Id, RenderElement,
                surface::WaylandSurfaceRenderElement,
            },
            gles::{GlesRenderer, GlesTexture},
            utils::CommitCounter,
        },
    },
    desktop::Window,
    utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::{pane::PaneId, state::Solium};

/// The pixel size a window's capture needs, at `scale`.
///
/// **Read from the window's own geometry and from nothing the warp is doing**,
/// which is the whole reason a cache keyed on it ever hits. `present.rs`'s
/// first rule is that a transform never changes real geometry: the matrix, the
/// deform and the animated `Frame::rect` are applied to this texture
/// afterwards, by `warp::mesh_part`, and not one of them is read here. So a
/// window bending through a genie is captured at the same size on every frame
/// of it.
///
/// The sizes that do change it are all one-offs — the client actually resizing,
/// its frame's insets changing, the window crossing onto a monitor at another
/// scale — and each costs exactly the one allocation it used to cost every
/// frame.
fn pixels(outer: Size<i32, Logical>, scale: f64) -> Size<i32, Physical> {
    (
        ((f64::from(outer.w) * scale).ceil() as i32).max(1),
        ((f64::from(outer.h) * scale).ceil() as i32).max(1),
    )
        .into()
}

/// The pixel size a client's self capture holds, at `scale`: rounded, as a
/// surface measures itself, and at least one pixel.
///
/// A capture sized with `ceil` is, at a fractional scale, one pixel wider than
/// the surfaces drawn into it -- 1149 logical at 1.25 is 1437 against 1436 --
/// and that last column is one no surface ever claims and no surface ever
/// draws into: `WaylandSurfaceRenderElement::opaque_regions` and
/// `render::elements` both round. Rounding here makes all of them
/// `round(logical * scale)`, the same function of the same numbers.
/// `.max(1)` for [`pixels`]' reason: a driver refuses a zero-sized
/// allocation, and `round` reaches zero half a pixel sooner than `ceil` does.
/// `tests::the_client_capture_is_measured_the_way_a_surface_measures_itself`.
pub(crate) fn client_pixels(outer: Size<i32, Logical>, scale: f64) -> Size<i32, Physical> {
    let rounded: Size<i32, Physical> = outer.to_physical_precise_round(scale);
    (rounded.w.max(1), rounded.h.max(1)).into()
}

/// What a slot's self input is a capture of. `Region` is not here: a
/// region's own pixels are a crop of the layer that draws it, which waits
/// for P15 (`effect::plan::tests::a_region_self_rule_is_refused_until_p15`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum PartSource<'a> {
    /// The client's own surface tree, no popups, no frame.
    Client(&'a Window),
    /// A window's popups, at the rectangle they cover.
    Popups(&'a Window),
    /// One of a pane style's layers, at its canvas: the pane grown by its
    /// bleed.
    Layer(PaneId, usize),
    /// The whole pane, frame and client, no popups.
    Pane(&'a Window),
    /// A scripted surface's instance on one monitor.
    Surface(crate::scripted::SurfaceId, &'a smithay::output::Output),
    /// A client's layer surface.
    LayerShell(&'a smithay::desktop::LayerSurface),
}

/// Where a job's target is held: a pane's capture of a kind, or a slot's
/// self input in the effect store (Ruling 16).
/// `tests::a_slots_capture_is_held_in_the_store`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HeldIn {
    Pane(PaneId, crate::keyed::Kind),
    Slot(crate::effect::plan::Owner, crate::effect::rules::Slot),
}

/// One part, padded by `pad` physical pixels on every side, for a slot's
/// self input: drawn from the elements its part is drawn from, moved so the
/// part's corner is at `(pad, pad)`, and keyed on them, so it is drawn again
/// only when the part commits (Ruling 16). `None` when the part has nothing
/// to draw, as a client with no buffer yet.
/// `state::tests::real_client::the_self_capture_is_padded_by_the_effects_reach`,
/// `state::tests::real_client::a_self_rule_captures_the_client_once_until_it_commits`.
pub(crate) fn part_job(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    owner: crate::effect::plan::Owner,
    slot: crate::effect::rules::Slot,
    source: PartSource<'_>,
    scale: f64,
    pad: i32,
) -> Option<Job> {
    let size = part_size(state, &source, scale, pad)?;
    let corner: Point<i32, Physical> = (pad, pad).into();
    let elements: Vec<crate::render::Element> = match source {
        PartSource::Client(_) | PartSource::Popups(_) | PartSource::LayerShell(_) => {
            part_elements(renderer, &source, pad, scale)
                .into_iter()
                .map(crate::render::Element::Window2)
                .collect()
        }
        PartSource::Layer(pane, index) => {
            vec![crate::render::one_layer(
                state, renderer, pane, index, corner, scale,
            )?]
        }
        PartSource::Pane(window) => {
            crate::render::flat_window_elements_at(state, renderer, window, scale, corner)
        }
        PartSource::Surface(id, output) => {
            vec![crate::render::scripted_instance(
                state, renderer, id, output, corner, scale,
            )?]
        }
    };
    if elements.is_empty() {
        // Debug and not warn: a client with nothing mapped yet is ordinary.
        tracing::debug!(?source, "a part with a self effect had nothing to capture");
        return None;
    }
    let inputs = crate::keyed::Inputs::of(crate::keyed::Kind::Pane, size, scale, &elements);
    Some(Job {
        into: HeldIn::Slot(owner, slot),
        size,
        scale,
        elements,
        inputs,
    })
}

/// The padded target's size: the part's own pixels plus `pad` each side.
/// `state::tests::real_client::the_self_capture_is_padded_by_the_effects_reach`.
pub(crate) fn part_size(
    state: &Solium,
    source: &PartSource<'_>,
    scale: f64,
    pad: i32,
) -> Option<Size<i32, Physical>> {
    let own = part_pixels(state, source, scale)?;
    Some((own.w + 2 * pad, own.h + 2 * pad).into())
}

/// The part's own pixels, unpadded: the client's hole in its pane (its
/// tile's share for a client that committed more, #133), the popups'
/// rectangle, a layer's canvas, the pane's outer rectangle as a warp's
/// capture measures it, a scripted surface's area, a layer surface's
/// geometry. `state::tests::real_client::the_self_capture_is_padded_by_the_effects_reach`.
fn part_pixels(state: &Solium, source: &PartSource<'_>, scale: f64) -> Option<Size<i32, Physical>> {
    Some(match source {
        PartSource::Client(window) => {
            client_pixels(crate::render::flat(state, window)?.client.size, scale)
        }
        PartSource::Popups(window) => {
            client_pixels(crate::render::popups_covered(window)?.size, scale)
        }
        PartSource::Pane(window) => pixels(crate::render::flat(state, window)?.outer, scale),
        PartSource::Layer(pane, index) => client_pixels(
            crate::render::layer_canvas(state, *pane, *index)?.size,
            scale,
        ),
        PartSource::Surface(id, output) => client_pixels(
            crate::render::instance_area(state, *id, output)?.size,
            scale,
        ),
        PartSource::LayerShell(surface) => client_pixels(
            crate::render::layer_shell_geometry(state, surface)?.size,
            scale,
        ),
    })
}

/// The padded box a slot's run reads: [`part_size`]'s, the part inside it,
/// its mask's radii, and the reach that padded it. GPU-free, so the runs
/// have it whether or not a capture was drawn this pass.
/// `state::tests::real_client::every_wanted_slot_has_its_part_box_padded_by_its_reach`.
pub(crate) fn part_box(
    state: &Solium,
    source: &PartSource<'_>,
    scale: f64,
    pad: i32,
) -> Option<crate::effect::plan::PartBox> {
    let own = part_pixels(state, source, scale)?;
    Some(crate::effect::plan::PartBox::around(
        own,
        pad,
        crate::render::part_radii(state, source, scale),
    ))
}

/// The surface-backed parts' elements, their part's corner at `(pad, pad)`,
/// generic over the renderer so a key can be built on smithay's
/// `DummyRenderer` in a test: the client's own tree through
/// `render::client_piece`, its window geometry's corner at the pad, so a
/// client that draws its own shadow has it cut off, as the capture before
/// Phase 0's Task 23b did, and not rounded (`Clipped`: the mask rounds the
/// result, Ruling 16); the popups through `render::popup_elements`, the
/// corner of the rectangle they cover at the pad; a layer surface through its
/// own `render_elements`. Empty for the others.
/// `state::tests::real_client::a_self_rule_captures_the_client_once_until_it_commits`,
/// `state::tests::real_client::a_subsurface_commit_recaptures_its_client`.
pub(crate) fn part_elements<R>(
    renderer: &mut R,
    source: &PartSource<'_>,
    pad: i32,
    scale: f64,
) -> Vec<WaylandSurfaceRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let corner: Point<i32, Physical> = (pad, pad).into();
    let output_scale = Scale::from(scale);
    match source {
        PartSource::Client(window) => {
            let origin = corner - window.geometry().loc.to_physical_precise_round(scale);
            crate::render::client_piece(renderer, window, origin, output_scale)
        }
        PartSource::Popups(window) => {
            let Some(covered) = crate::render::popups_covered(window) else {
                return Vec::new();
            };
            let origin = corner - covered.loc.to_physical_precise_round(scale);
            crate::render::popup_elements(renderer, window, origin, output_scale, 1.0).0
        }
        PartSource::LayerShell(surface) => {
            AsRenderElements::render_elements(*surface, renderer, corner, output_scale, 1.0)
        }
        PartSource::Layer(..) | PartSource::Pane(_) | PartSource::Surface(..) => Vec::new(),
    }
}

/// One capture to draw: what, into which capture, how big.
///
/// Built by [`pane_job`], [`over_job`] or [`part_job`] while `render::prepare`
/// walks the panes and the slots, which can run Qt, and drawn by [`draw`] once
/// every job is built, which must not. `dev/fence-check.sh` draws two warps in
/// one pass, and `dev/present-check.sh` every warp it measures, a menu's among
/// them.
pub(crate) struct Job {
    /// Where the capture's target is held: on a pane, or in a slot's state in
    /// the effect store (Ruling 16).
    pub(crate) into: HeldIn,
    pub(crate) size: Size<i32, Physical>,
    pub(crate) scale: f64,
    pub(crate) elements: Vec<crate::render::Element>,
    /// What the capture is drawn from, built from `elements`: a capture that
    /// was last drawn from exactly this is kept ([`kept`]).
    pub(crate) inputs: crate::keyed::Inputs,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job")
            .field("into", &self.into)
            .field("size", &self.size)
            .field("scale", &self.scale)
            .field("elements", &self.elements.len())
            .finish()
    }
}

/// `window` flat at its pane's size, frame and all: a warp's capture.
///
/// The job's size is what a caller maps texture coordinates back onto the
/// window's own rectangle with. It is [`crate::render::flat`]'s -- the pane's
/// outer rectangle, which is what the warp's mesh is built over, and which for
/// a tiled client that committed more than its tile is the tile (#133).
///
/// The target it is drawn into belongs to `pane` and outlives the frame; see
/// [`crate::keyed::Captures`]. The pane is passed in rather than looked up
/// with `Panes::id_of`, because the one caller is iterating panes and already
/// holds it — and because a capture with nowhere to keep its target would be
/// back to allocating one a frame, which is a case worth not having rather
/// than one worth handling.
pub(crate) fn pane_job(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    pane: PaneId,
    window: &Window,
    scale: f64,
) -> Option<Job> {
    let outer = crate::render::flat(state, window)?.outer;

    // Built at the origin rather than at the window's position: the texture is
    // the window's own space, and where it ends up on screen is the warp's
    // business.
    let elements = crate::render::flat_window_elements(state, renderer, window, scale);
    if elements.is_empty() {
        tracing::warn!("a warped window had nothing to draw offscreen");
        return None;
    }
    let (kind, size) = (crate::keyed::Kind::Pane, pixels(outer, scale));
    let inputs = crate::keyed::Inputs::of(kind, size, scale, &elements);
    Some(Job {
        into: HeldIn::Pane(pane, kind),
        size,
        scale,
        elements,
        inputs,
    })
}

/// A warped pane's popups, captured at the rectangle they cover, and their
/// part of the pane's unit square. `None` when it has none open.
///
/// The rectangle may reach past the window, so a menu is captured whole and
/// not cut at the window's edge (`state::tests::a_popup_past_the_window_is_captured_whole`);
/// it is keyed on the popups' own surfaces, so a menu that commits during a
/// flight is captured again
/// (`state::tests::a_commit_on_a_popup_makes_the_popups_capture_stale`).
/// `render::panes` draws it as a second warp in front of the pane's, through
/// the same matrix and deform, meshed over that part
/// (`render::tests::a_warped_panes_popups_are_in_front_of_it`,
/// `render::tests::the_popups_part_is_their_rectangle_over_the_pane`).
pub(crate) fn over_job(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    pane: PaneId,
    window: &Window,
    scale: f64,
) -> Option<(Job, crate::warp::UnitRect)> {
    let flat = crate::render::flat(state, window)?;
    let corner = Point::<i32, Logical>::from((flat.insets.left, flat.insets.top));
    let (_, covered) =
        crate::render::popup_elements(renderer, window, (0, 0).into(), Scale::from(scale), 1.0);
    let covered = covered?;
    // Drawn so the covered rectangle's corner is the texture's: a menu drawn
    // anywhere else is cut at the texture's edge, which
    // `dev/present-check.sh`'s `menu` case measures.
    let origin =
        (Point::<i32, Logical>::from((0, 0)) - covered.loc).to_physical_precise_round(scale);
    let (popups, _) =
        crate::render::popup_elements(renderer, window, origin, Scale::from(scale), 1.0);
    let elements: Vec<crate::render::Element> = popups
        .into_iter()
        .map(crate::render::Element::Window2)
        .collect();
    let (kind, size) = (crate::keyed::Kind::Over, pixels(covered.size, scale));
    let inputs = crate::keyed::Inputs::of(kind, size, scale, &elements);
    let part = crate::render::over_part(flat.outer, corner, covered);
    Some((
        Job {
            into: HeldIn::Pane(pane, kind),
            size,
            scale,
            elements,
            inputs,
        },
        part,
    ))
}

/// Draw every job into its pane's capture: the carrier bound once, a frame per
/// capture, every element list already built so nothing here runs Qt.
///
/// Each job comes with what it becomes once drawn (`then`), handed back beside
/// its texture and the capture's id and commit, the commit moved by this draw;
/// a job that could not be drawn is left out, and its window is drawn the way
/// it would be with no capture at all. `dev/fence-check.sh`
/// checks the pictures, two warps on one carrier, byte for byte with the
/// fence wait on and off.
pub(crate) fn draw<T>(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    jobs: Vec<(Job, T)>,
) -> Vec<(T, GlesTexture, Id, CommitCounter)> {
    // Nothing captured this pass, which is every pass of an unstyled,
    // unwarped desktop: no carrier bound, no framebuffer released, no context
    // made current, as before captures went through the pool. Only targets
    // given back are swept, and `sweep` touches GL only when there are some.
    // `render::tests::a_pane_neither_warped_nor_styled_wants_no_capture`.
    if jobs.is_empty() {
        state.pool.sweep(renderer);
        return Vec::new();
    }
    let mut done = Vec::with_capacity(jobs.len());
    match state.pool.carrier(renderer) {
        None => tracing::warn!("no carrier to draw this pass's captures on"),
        Some(mut carrier) => {
            let wait = crate::dev::fence_wait();
            match renderer.bind(&mut carrier) {
                Err(err) => tracing::warn!(?err, "could not bind the capture carrier"),
                Ok(mut bound) => {
                    // Nothing that touches a QML scene may run in here; every
                    // list was built before it. See `qml::no_frame_in_flight`.
                    let _frame = crate::qml::frame_in_flight();
                    for (job, then) in jobs {
                        let target = {
                            let (panes, store, pool) =
                                (&mut state.panes, &mut state.store, &mut state.pool);
                            capture_in(panes, store, &job.into).and_then(|capture| {
                                capture.target_for(pool, &mut crate::pool::Gl(renderer), job.size)
                            })
                        };
                        let Some(target) = target else {
                            tracing::warn!(size = ?job.size, "no target for a capture");
                            continue;
                        };
                        let drawn = match crate::pool::frame_for(renderer, &mut bound, &target) {
                            Err(err) => {
                                tracing::warn!(?err, "could not open a capture's frame");
                                false
                            }
                            Ok(mut frame) => {
                                let stamp = state.timer.as_mut().map(|timer| {
                                    timer.open_in(&mut frame, crate::gputime::Region::Capture)
                                });
                                if let Err(err) = crate::pool::paint(
                                    &mut frame,
                                    job.size,
                                    &job.elements,
                                    job.scale,
                                ) {
                                    tracing::warn!(?err, "a window did not render offscreen");
                                }
                                if let (Some(timer), Some(stamp)) = (state.timer.as_mut(), stamp) {
                                    timer.close_in(&mut frame, stamp);
                                }
                                // See `settle`: waited on by default, and the
                                // window full of garbage that sampling an
                                // unfinished texture can show is why.
                                settle(frame.finish(), wait, "a capture")
                            }
                        };
                        if drawn {
                            crate::pacing::captured();
                            if let Some(capture) =
                                capture_in(&mut state.panes, &mut state.store, &job.into)
                            {
                                capture.drawn(target.clone(), job.inputs);
                                done.push((
                                    then,
                                    target.texture().clone(),
                                    capture.id().clone(),
                                    capture.commit(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    // The carrier's framebuffer, or a target's, is still bound: on a backend
    // that renders into an EGL surface nothing binds 0 again, and every later
    // frame would land in a texture nobody shows. See `release_framebuffer`.
    crate::warp::release_framebuffer(renderer);
    state.pool.sweep(renderer);
    done
}

/// The capture `job` would draw, when it is already drawn from exactly this:
/// no frame, no GPU work, and the same id and commit as last pass.
/// `keyed::tests::a_capture_never_drawn_is_stale` and
/// `state::tests::real_client::a_capture_whose_surface_tree_has_not_committed_is_not_drawn_again`
/// are the property; `SOLIUM_RECAPTURE=always` turns it off
/// (`dev::tests::recapture_always_is_asked_for_by_name`).
pub(crate) fn kept(state: &mut Solium, job: &Job) -> Option<(GlesTexture, Id, CommitCounter)> {
    if crate::dev::recapture_always() {
        return None;
    }
    let capture = capture_in(&mut state.panes, &mut state.store, &job.into)?;
    if capture.stale(&job.inputs) {
        return None;
    }
    Some((
        capture.target()?.texture().clone(),
        capture.id().clone(),
        capture.commit(),
    ))
}

/// A pane capturing nothing this pass gives every capture back.
/// `keyed::tests::a_pane_that_stops_warping_gives_the_texture_back`.
pub(crate) fn release(state: &mut Solium, pane: PaneId) {
    let (panes, pool) = (&mut state.panes, &mut state.pool);
    if let Some(held) = panes.get_mut(pane) {
        held.captures_mut().keep(&[], pool);
    }
}

/// The capture a job is held in: a pane's of its kind, or a slot's self input
/// in the effect store, made by `Store::slot_mut` before its job was built.
/// `None` for a pane that has gone or a slot no longer in the store.
/// `tests::a_slots_capture_is_held_in_the_store`.
fn capture_in<'s>(
    panes: &'s mut crate::pane::Panes,
    store: &'s mut crate::effect::store::Store,
    into: &HeldIn,
) -> Option<&'s mut crate::keyed::Capture> {
    match into {
        HeldIn::Pane(pane, kind) => panes
            .get_mut(*pane)
            .map(|held| held.captures_mut().get_mut(*kind)),
        HeldIn::Slot(owner, slot) => store.get_mut(owner, *slot).map(|state| &mut state.input),
    }
}

/// Whether a finished offscreen frame may be sampled: the compositor's
/// `SyncPoint`, or a counting stand-in in the tests.
pub(crate) trait Finished {
    /// Wait for the GPU, and say whether that worked.
    fn waited(&self) -> bool;
}

impl Finished for smithay::backend::renderer::sync::SyncPoint {
    fn waited(&self) -> bool {
        match self.wait() {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(?err, "waiting for an offscreen draw failed");
                false
            }
        }
    }
}

/// Whether an offscreen draw that `finish` answered may be sampled.
///
/// **Waited on, unless `SOLIUM_FENCE_WAIT=off`.** The fence says when the GPU
/// has finished drawing into the texture. Every reader of a capture today is
/// on the context that wrote it, where GL already orders the read after the
/// write, so the wait buys nothing the GPU does not do anyway; it is kept as
/// the default until #59 lands (§6.5, C2). wirecheck's cases 11c and 11d
/// prove the order on the GPU, through smithay and through raw GL.
/// `tests::a_capture_waits_on_the_cpu_only_when_asked`,
/// `tests::a_wait_that_fails_is_a_failed_capture`,
/// `tests::a_frame_that_did_not_finish_is_a_failed_capture_either_way`.
/// Dropping an `EGLFence` is `eglDestroySync`, not a wait.
pub(crate) fn settle<F: Finished, E: std::fmt::Debug>(
    finished: Result<F, E>,
    wait: crate::dev::FenceWait,
    what: &str,
) -> bool {
    match finished {
        Err(err) => {
            tracing::warn!(?err, what, "an offscreen draw did not finish");
            false
        }
        Ok(sync) => match wait {
            crate::dev::FenceWait::Cpu => sync.waited(),
            crate::dev::FenceWait::Skip => true,
        },
    }
}

/// One monitor's worth of picture, drawn into a texture of its own.
///
/// The nested backend's multi-monitor mode. On the hardware each output has its
/// own buffer and its own page flip; here there is one window, so each virtual
/// monitor is rendered into its own texture exactly as it would be into its own
/// buffer, and the window draws those side by side.
///
/// That is not a shortcut around the real thing — it is the real thing with the
/// scanout replaced. Every per-output path runs for each of them: its own
/// elements built at its own origin, its own layer map, its own work area. What
/// it cannot simulate is a second *pipeline*, which is `tty.rs`'s half of the
/// problem.
pub(crate) struct Screens {
    /// One texture per monitor, kept across frames. Allocating these per frame
    /// would be several hundred megabytes a second of texture churn on a
    /// development path, and would make the leak check in `dev/leak.sh` read
    /// like a leak.
    ///
    /// The same reasoning, and for a long time the only place it was written
    /// down: the pane's pooled captures (`keyed::Captures`) are it applied to
    /// the per-window path, where there is one per warped window rather than
    /// one per monitor.
    textures: Vec<(Size<i32, Physical>, GlesTexture)>,
}

impl std::fmt::Debug for Screens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screens")
            .field("count", &self.textures.len())
            .finish()
    }
}

impl Screens {
    pub(crate) fn new() -> Self {
        Self {
            textures: Vec::new(),
        }
    }

    /// The texture for monitor `index`, at `size`, creating or replacing it if
    /// the size is not what it was.
    fn texture(
        &mut self,
        renderer: &mut GlesRenderer,
        index: usize,
        size: Size<i32, Physical>,
    ) -> Option<&mut GlesTexture> {
        while self.textures.len() <= index {
            let buffer: Size<i32, BufferCoords> = (size.w.max(1), size.h.max(1)).into();
            let texture = renderer.create_buffer(Fourcc::Abgr8888, buffer).ok()?;
            self.textures.push((size, texture));
        }
        if self
            .textures
            .get(index)
            .is_some_and(|(had, _)| *had != size)
        {
            let buffer: Size<i32, BufferCoords> = (size.w.max(1), size.h.max(1)).into();
            let texture = renderer.create_buffer(Fourcc::Abgr8888, buffer).ok()?;
            *self.textures.get_mut(index)? = (size, texture);
        }
        self.textures.get_mut(index).map(|(_, texture)| texture)
    }

    /// Draw one monitor and hand back its texture.
    pub(crate) fn draw(
        &mut self,
        state: &mut Solium,
        renderer: &mut GlesRenderer,
        prepared: &crate::render::Prepared,
        index: usize,
        screen: Rectangle<i32, Logical>,
        scale: f64,
    ) -> Option<(GlesTexture, Size<i32, Physical>)> {
        let size: Size<i32, Physical> = (
            ((f64::from(screen.size.w) * scale).ceil() as i32).max(1),
            ((f64::from(screen.size.h) * scale).ceil() as i32).max(1),
        )
            .into();

        // Built before the texture is bound, because building can bind
        // framebuffers of its own -- the warp pass -- and a bind underneath a
        // bind redirects the whole picture. Same reason `Prepared` exists.
        let elements = crate::render::elements(
            state,
            renderer,
            prepared,
            crate::render::Picture::screen(screen, scale),
        );

        let mut texture = self.texture(renderer, index, size)?.clone();
        let drawn = {
            let mut framebuffer = match renderer.bind(&mut texture) {
                Ok(framebuffer) => framebuffer,
                Err(err) => {
                    tracing::warn!(?err, index, "could not bind a monitor's buffer");
                    crate::warp::release_framebuffer(renderer);
                    return None;
                }
            };
            let _frame = crate::qml::frame_in_flight();
            match renderer.render(&mut framebuffer, size, Transform::Normal) {
                Err(err) => {
                    tracing::warn!(?err, index, "could not render a monitor");
                    false
                }
                Ok(mut frame) => {
                    let stamp = state.timer.as_mut().map(|timer| {
                        timer.open_in(
                            &mut frame,
                            crate::gputime::Region::Output(u8::try_from(index).unwrap_or(u8::MAX)),
                        )
                    });
                    // The same background the backends clear to, so an empty
                    // monitor looks like an empty monitor rather than a hole.
                    frame
                        .clear(
                            Color32F::new(0.05, 0.05, 0.06, 1.0),
                            &[Rectangle::from_size(size)],
                        )
                        .unwrap_or_else(|err| tracing::warn!(?err, "clearing a monitor failed"));

                    let whole = [Rectangle::from_size(size)];
                    // Reversed: this list is topmost-first, and drawing it in
                    // that order onto a cleared buffer paints the top of the
                    // stack first and everything else over it.
                    for element in elements.iter().rev() {
                        let source = element.src();
                        let destination = element.geometry(Scale::from(scale));
                        if let Err(err) = element.draw(&mut frame, source, destination, &whole, &[])
                        {
                            tracing::warn!(?err, index, "an element did not render");
                        }
                    }

                    if let (Some(timer), Some(stamp)) = (state.timer.as_mut(), stamp) {
                        timer.close_in(&mut frame, stamp);
                    }
                    settle(frame.finish(), crate::dev::fence_wait(), "a monitor")
                }
            }
        };

        crate::warp::release_framebuffer(renderer);
        drawn.then_some((texture, size))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use smithay::utils::{Logical, Physical, Size};

    use super::{Finished, settle};
    use super::{client_pixels, pixels};

    /// An ordinary window, the same one `qml::paint`'s tests measure and the
    /// one the pool's budget is argued from: 1150 x 850 x 4 = 3.9 MB.
    fn window() -> Size<i32, Logical> {
        (1150, 850).into()
    }

    /// What a texture costs, at four bytes a pixel.
    fn bytes(size: Size<i32, Physical>) -> i64 {
        i64::from(size.w) * i64::from(size.h) * 4
    }

    /// **The scale is not separately part of a target's size, and does not
    /// need to be.**
    ///
    /// `qml::paint`'s `Drawn` carries the pixels *and* the scale because one
    /// buffer size holds two different pictures at two scales — the host is
    /// handed both and lays the scene out from the pair. A target a pane's
    /// capture holds (`keyed::Captures`) is not a picture: `draw` clears and
    /// redraws the whole target whenever it draws it, so a buffer with the
    /// right number of pixels is the right buffer whatever last drew into it.
    /// Whether it is drawn again at all is `keyed::Inputs`' question
    /// (`keyed::tests::a_capture_of_another_kind_at_the_same_size_is_stale`).
    ///
    /// The scale still decides the target, through the size it multiplies,
    /// which is what makes a window crossing to a 2x monitor a miss rather
    /// than a window drawn at half its resolution.
    #[test]
    fn a_window_crossing_to_a_2x_monitor_asks_for_a_different_buffer() {
        let outer = window();
        assert_eq!(pixels(outer, 1.0), Size::from((1150, 850)));
        assert_eq!(pixels(outer, 2.0), Size::from((2300, 1700)));
        // A fractional scale rounds up, so the texture is never short of a row.
        assert_eq!(pixels(outer, 1.5), Size::from((1725, 1275)));

        // The numbers the pool's budget is argued from.
        assert_eq!(bytes(pixels(outer, 1.0)), 3_910_000);
        assert_eq!(bytes(pixels(outer, 2.0)), 15_640_000);

        /// Counts the textures it makes.
        #[derive(Debug)]
        struct Made(u32);
        impl crate::pool::Alloc for Made {
            type Tex = u32;
            fn make(
                &mut self,
                _size: Size<i32, Physical>,
                _format: crate::pool::Format,
            ) -> Option<u32> {
                self.0 += 1;
                Some(self.0)
            }
            fn fbo(&mut self, _texture: &u32) -> Option<u32> {
                Some(self.0)
            }
        }

        let kind = crate::keyed::Kind::Pane;
        let mut captures = crate::keyed::Captures::<u32>::default();
        let (mut pool, mut made) = (crate::pool::Pool::new(0), Made(0));
        for scale in [1.0, 1.0, 2.0, 2.0] {
            let target = captures
                .get_mut(kind)
                .target_for(&mut pool, &mut made, pixels(outer, scale))
                .expect("a counting allocator cannot fail");
            let inputs = crate::keyed::Inputs::of::<
                smithay::backend::renderer::element::solid::SolidColorRenderElement,
            >(kind, pixels(outer, scale), scale, &[]);
            captures.get_mut(kind).drawn(target, inputs);
        }
        assert_eq!(
            made.0, 2,
            "the two monitors did not each get a buffer of their own"
        );
    }

    /// A window of no size is still given a texture rather than a zero-sized
    /// allocation the driver would refuse.
    #[test]
    fn a_window_with_no_size_still_asks_for_a_pixel() {
        assert_eq!(pixels((0, 0).into(), 1.0), Size::from((1, 1)));
        assert_eq!(pixels((1, 1).into(), 0.1), Size::from((1, 1)));
    }

    /// **The client capture rounds, and the warp still ceils.**
    ///
    /// Not a preference between two roundings: `pass::covers` asks whether the
    /// client's surfaces covered the capture, and a surface's opaque region is
    /// sized with `to_i32_round` (`element/surface.rs:353-356`). A capture one
    /// pixel wider than that has a column no surface claims and no surface
    /// draws into, so `covers` is false, `opaque_of` is `None`, and every
    /// rounded window on a fractional-scale output gives up its opaque region
    /// permanently -- back to claiming none of it, with nothing on screen to
    /// say so.
    ///
    /// 1149 at 1.25 is the case `render::elements` records: 1436.25, which
    /// ceils to 1437 and rounds to 1436. Both are asserted, in one test,
    /// because the bug is the *difference* between them and a test of either
    /// alone would not have caught it.
    ///
    /// What this cannot check is the thing that matters: whether a real
    /// client's real opaque regions then cover a real capture. Nothing here
    /// can build one. It pins that the two functions agree on the number, which
    /// is the half that was wrong.
    #[test]
    fn the_client_capture_is_measured_the_way_a_surface_measures_itself() {
        let width: Size<i32, Logical> = (1149, 850).into();
        assert_eq!(pixels(width, 1.25).w, 1437, "the warp still ceils");
        assert_eq!(
            client_pixels(width, 1.25).w,
            1436,
            "and the client capture rounds, as `render::elements` and \
             `WaylandSurfaceRenderElement::opaque_regions` both do"
        );
        // And a fraction on the OTHER side of a half, because 1149 x 1.25 is
        // 1436.25 and truncating gives 1436 too -- so the case above cannot
        // tell rounding from flooring, and a later "simplification" to
        // `to_i32_floor` would pass it while re-opening the one-pixel
        // disagreement in the other direction.
        let over: Size<i32, Logical> = (1151, 850).into();
        assert_eq!(
            client_pixels(over, 1.25).w,
            1439,
            "1151 x 1.25 is 1438.75, which rounds up -- flooring gives 1438 \
             and puts the capture a pixel inside `dst` again"
        );

        // Where there is nothing to disagree about, they agree.
        for scale in [1.0, 2.0] {
            assert_eq!(pixels(width, scale), client_pixels(width, scale));
        }
    }

    /// **A slot's capture is held in the store**, in the state its slot
    /// was given this pass; a slot not in the store, or a pane that has
    /// gone, holds none.
    #[test]
    fn a_slots_capture_is_held_in_the_store() {
        use crate::effect::plan::{Owner, PaneSlot};
        use crate::effect::rules::{Origin, RuleKey, Slot};
        use crate::effect::store::Store;
        let owner = Owner::Pane(crate::pane::PaneId::from_raw(3), PaneSlot::Client);
        let rule = RuleKey {
            origin: Origin::User,
            index: 0,
            generation: 1,
        };
        let (mut panes, mut store) = (crate::pane::Panes::default(), Store::default());
        let into = super::HeldIn::Slot(owner.clone(), Slot::Replace);
        assert!(super::capture_in(&mut panes, &mut store, &into).is_none());
        let id = store
            .slot_mut(&owner, Slot::Replace, rule)
            .input
            .id()
            .clone();
        assert_eq!(
            super::capture_in(&mut panes, &mut store, &into).map(|capture| capture.id().clone()),
            Some(id)
        );
        let pane = super::HeldIn::Pane(crate::pane::PaneId::from_raw(3), crate::keyed::Kind::Pane);
        assert!(super::capture_in(&mut panes, &mut store, &pane).is_none());
    }

    /// A fence that counts how often it is waited on, and answers as told.
    #[derive(Debug)]
    struct Counted {
        waits: Rc<Cell<u32>>,
        answer: bool,
    }

    impl Finished for Counted {
        fn waited(&self) -> bool {
            self.waits.set(self.waits.get() + 1);
            self.answer
        }
    }

    /// **A capture waits on the CPU only when asked to.** Skipped, the fence is
    /// dropped unwaited and the capture is still sampleable: GL orders the
    /// reads after the writes on the one context (wirecheck's cases 11c and
    /// 11d, Task 9).
    #[test]
    fn a_capture_waits_on_the_cpu_only_when_asked() {
        use crate::dev::FenceWait;
        let waits = Rc::new(Cell::new(0));
        let fence = || Counted {
            waits: Rc::clone(&waits),
            answer: true,
        };
        assert!(settle::<_, ()>(Ok(fence()), FenceWait::Cpu, "a test"));
        assert_eq!(waits.get(), 1, "the default waits");
        assert!(settle::<_, ()>(Ok(fence()), FenceWait::Skip, "a test"));
        assert_eq!(waits.get(), 1, "switched off, nothing waits");
    }

    /// A wait that fails is a failed capture, as it was before both waits went
    /// through `settle`.
    #[test]
    fn a_wait_that_fails_is_a_failed_capture() {
        let waits = Rc::new(Cell::new(0));
        let failing = Counted {
            waits: Rc::clone(&waits),
            answer: false,
        };
        assert!(!settle::<_, ()>(
            Ok(failing),
            crate::dev::FenceWait::Cpu,
            "a test"
        ));
    }

    /// And a frame that did not finish is a failed capture either way.
    #[test]
    fn a_frame_that_did_not_finish_is_a_failed_capture_either_way() {
        use crate::dev::FenceWait;
        assert!(!settle::<Counted, &str>(
            Err("no"),
            FenceWait::Cpu,
            "a test"
        ));
        assert!(!settle::<Counted, &str>(
            Err("no"),
            FenceWait::Skip,
            "a test"
        ));
    }
}
