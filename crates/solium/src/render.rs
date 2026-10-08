//! Building the frame's render elements, through the presentation transform.
//!
//! This is the only place that knows a window may be drawn somewhere other
//! than where it lives. Every mode — overview, switcher, peek, genie — is
//! expressed by setting a target and reaches the screen through this function,
//! which is why they cannot animate inconsistently.
//!
//! Drawn with GLES: the element set below is concrete on `GlesRenderer`, and
//! so is everything that draws through it. What a Vulkan backend would have to
//! replace is listed in `docs/spikes/2026-08-27-vulkan-on-smithay.md`, under
//! "What a Vulkan port costs now".

use std::sync::Mutex;

use smithay::{
    backend::renderer::{
        ImportAll, Renderer,
        element::{
            AsRenderElements, Id, Kind,
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::{CropRenderElement, RescaleRenderElement},
        },
        gles::{GlesRenderer, GlesTexProgram, GlesTexture},
        utils::CommitCounter,
    },
    desktop::{LayerSurface, PopupManager, Window, layer_map_for_output},
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::compositor::with_states,
};

use crate::{
    effect::{
        mask::{Mask, RegionSource as _},
        plan::{PaneSlot, Slots},
        rules::{MaskKind, Slot},
    },
    layer,
    offscreen::PartSource,
    pane::Pane,
    present,
    stack::{Band, Owner},
    state::Solium,
    style::Depth,
};
use solium_effects::fragment::Corners;

render_elements! {
    /// Everything Solium can draw.
    ///
    /// `Window` is a client's surface, placed wherever its presentation says.
    /// `Chrome` is a texture the compositor rendered itself: window frames,
    /// drawn from QML. `Warped` is a texture through four arbitrary corners,
    /// which is how anything that is not a rectangle reaches the screen.
    ///
    /// Concrete on `GlesRenderer` rather than generic, because `Warped` can
    /// only be GLES — see `warp.rs`. The cost is recorded in the spike: a
    /// Vulkan backend needs its own element set, not just its own warp.
    pub(crate) Element<=GlesRenderer>;
    Window = RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    /// A tiled client's surface, cut to its tile because it committed more
    /// than the tile has. See [`fit`] — every other client surface, and every
    /// popup, is a `Window`.
    Tiled = CropRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
    Chrome = MemoryRenderBufferRenderElement<GlesRenderer>,
    Warped = crate::warp::Warp,
    /// A surface drawn straight, with no rescale wrapper: the offscreen pass
    /// draws at real size, so there is nothing to scale.
    Window2 = WaylandSurfaceRenderElement<GlesRenderer>,
    /// A flat colour. Two things draw one: the lock screen's backdrop, which
    /// has to be a real element rather than a clear colour because the lock
    /// client's surface is composited on top of it, and the fill a window that
    /// has gone is drawn with where its client left nothing (`remains::FILL`).
    Solid = smithay::backend::renderer::element::solid::SolidColorRenderElement,
    /// Something already drawn into a texture of its own.
    ///
    /// Two things produce these and they have nothing in common but the shape.
    /// The nested backend's multi-monitor mode draws a whole monitor into one,
    /// because there is one window and several screens to put in it — on the
    /// hardware a monitor is a scanout buffer and never an element; see
    /// `offscreen::Screens`. And a shell surface on the GPU path is a texture
    /// too: Qt rendered it into a buffer the compositor allocated, so there is
    /// nothing to upload and nothing that is a memory buffer. See `surface.rs`.
    Screen = smithay::backend::renderer::element::texture::TextureRenderElement<GlesTexture>,
    /// A surface of a window whose client has gone, drawn from the texture the
    /// renderer had imported for it. See `crate::remains`.
    Remains = RescaleRenderElement<crate::remains::Surface>,
    /// The same, cut to the tile the window left, as a live `Tiled` is.
    RemainsTiled = CropRenderElement<RescaleRenderElement<crate::remains::Surface>>,
    /// A client surface drawn through its client's rounded rectangle, in its
    /// own place: rounding with no capture. See `crate::clip`.
    ClippedWindow = RescaleRenderElement<crate::clip::Clipped>,
    /// The same, cut to its tile.
    ClippedTiled = CropRenderElement<RescaleRenderElement<crate::clip::Clipped>>,
    /// The same inside a warp's capture, drawn at real size as `Window2` is.
    Clipped2 = crate::clip::Clipped,
    /// An effect's result in its slot, cut by its part's mask. See
    /// `crate::effect::element`.
    Effect = crate::effect::element::EffectElement,
}

/// The two questions that together mean "will a later frame differ from this
/// one", asked of a QML scene.
///
/// Implemented by the two things the compositor draws from QML, which are
/// otherwise unrelated: a window frame and a scripted surface. It is a trait
/// rather than an inherent method on each so that [`Drawn::drawing`] can hold
/// the order and the combination for both — and so the tests can drive them
/// with a stand-in, which is the only way they can be driven at all. Nothing
/// else should call these: reaching for one at a call site is how the order
/// goes wrong, and reaching for only one is how the answer goes wrong.
pub(crate) trait Painted {
    /// Qt's dirty flag: has something the scene renders actually changed.
    ///
    /// **Spent by drawing.** Qt sets it when an animation step or a property
    /// write changes the scene, and `Scene::render` — and the GPU path's
    /// render — clear it again. So it only means anything *before* the draw
    /// that takes it. Afterwards it means "did the draw I have just done leave
    /// anything behind", and the answer to that is always no.
    fn something_new_to_draw(&self) -> bool;

    /// Whether an animation in the scene is still running.
    ///
    /// The half the dirty flag cannot answer, and the residual defect after
    /// `Drawn` was introduced. Qt raises `dirty` on a *change*, and a running
    /// animation does not produce one every tick: the tick that starts a
    /// `Behavior` has not moved the property yet, and an interpolation between
    /// two nearby values spends several ticks landing on the value it already
    /// had. Measured against Qt 6.11.2 from a settled scene, every shipped
    /// decoration has at least one clean tick before its animation finishes,
    /// and `reveal.qml` and `reactive.qml` are clean on the very frame that
    /// starts theirs — so on the dirty flag alone their animation took no step
    /// at all unless something unrelated happened to damage the screen. Which
    /// is "sometimes", and is exactly what it looked like on the hardware.
    ///
    /// **What neither of these two can see, and what should not be added
    /// here.** Together they answer "will a later frame differ from this one",
    /// and they answer it correctly — but an animation advanced past its own
    /// end in a single tick satisfies both of them perfectly. `dirty` is
    /// raised, because the value really did change; this reads `true` on the
    /// frame the `Behavior` starts and `false` a tick later, because the
    /// animation really has finished. The loop then asks for exactly the frames
    /// it should and every one of them shows the final value, which on the
    /// hardware is a titlebar that does not slide out — it is simply there.
    /// That was the third defect in this area and all of it was in the
    /// animation *clock*; see `CompositorAnimationDriver` in `qml/host.cpp`.
    /// The rate an animation advances at is not a question either of these is
    /// being asked, and `dev/wirecheck`'s appear case is what asks it, against
    /// a real Qt, by reading the animated value out of the object tree.
    fn animation_in_flight(&self) -> bool;
}

/// What one draw produced: the element, and whether there is more to come.
///
/// One value out of one call rather than a draw followed by a question, and
/// that is the entire point of the type. The question used to be a second call
/// at the call site, made *after* the draw had already spent the flag, so it
/// always answered no. `state.redraw` was therefore never set, the next frame
/// was never asked for, and a decoration's own animation advanced only when
/// something else happened to damage the screen: a pulse that ran while the
/// mouse moved and stopped dead the moment it did, a tooltip that appeared in
/// jerks, and — measured, nested, with nothing else on screen — zero frames in
/// sixty seconds.
///
/// Handing the answer back from the draw makes that shape unrepresentable.
/// There is no second query left to put in the wrong place.
///
/// The flag alone was still the wrong *question*, which is a separate defect
/// from asking the right one too late; see [`Painted::animation_in_flight`].
#[derive(Default)]
pub(crate) struct Drawn {
    /// What to put in the frame, if there is anything to put in it.
    pub(crate) element: Option<Element>,
    /// Whether the scene still has somewhere to go, read before this draw.
    pub(crate) animating: bool,
}

impl Drawn {
    /// Read the flag, then draw — in that order, once, in one place.
    ///
    /// A function taking the draw as a closure rather than two statements at
    /// each call site, because the order *is* the defect and two statements can
    /// be written either way round. Here they cannot.
    ///
    /// Generic over what is being drawn so that a test can stand in something
    /// whose flag behaves the way Qt's does — set by the animation tick,
    /// cleared by the draw — with no renderer and no Qt host. That seam is part
    /// of the fix and not incidental to it: every real caller needs a live
    /// `GlesRenderer` and a running Qt, neither of which exists under
    /// `cargo test`, so before this there was nothing here a test could reach.
    /// Which is exactly how the shipped order survived for weeks.
    pub(crate) fn drawing<P: Painted>(
        painted: &mut P,
        draw: impl FnOnce(&mut P) -> Option<Element>,
    ) -> Self {
        // Before, and it has to stay before. See [`Painted`].
        //
        // Both questions, because neither one is the whole answer. The flag
        // misses every tick of an animation that did not move a rendered
        // property, which includes the tick that starts one. The animation
        // census misses a change that is not an animation at all -- a new
        // title, a colour with no `Behavior` on it -- and it also misses the
        // last tick of an animation, which finishes *and* leaves the final
        // value to be drawn: the census reads false there while the flag reads
        // true. `||` and not either half, and the order is the cheap question
        // first, since it short-circuits the walk on every frame that is
        // redrawing anyway.
        //
        // Bracketed for `SOLIUM_PACING`, and this is the one place either
        // question is asked, which is what makes the measurement whole:
        // `animation_in_flight` walks a scene's QML object tree looking for a
        // running animation, and it is asked once per scene per output per
        // frame. ~0.6 µs for a settled 21-object scene is a fine number and a
        // frightening one, depending entirely on how many scenes there are and
        // how fast the screen is — so the compositor should be able to say,
        // rather than have it argued about. The span closes before the draw, or
        // Qt's rendering would be charged to the census.
        let animating = {
            let _census = crate::pacing::span(crate::pacing::Phase::Census);
            painted.something_new_to_draw() || painted.animation_in_flight()
        };
        crate::pacing::scene_asked(animating);
        Self {
            element: draw(painted),
            animating,
        }
    }
}

/// Textures captured for this frame: one per deformed window, and one more for
/// its popups while it has any open.
///
/// Such a window is drawn into a texture of its own first. That pass binds a
/// framebuffer, so it cannot happen while the output's buffer is already
/// bound: it leaves GL pointing at the texture, and the whole frame --
/// including the deformed window -- lands there instead of on screen, which
/// looks exactly like a compositor that has frozen. So captures happen in
/// their own pass, before the backend binds anything, and `elements` only
/// spends what this collected.
///
/// **That is why a capture is not drawn from inside `panes`**, which is where
/// the warp is placed and would be the obvious place to draw it. `elements` is
/// called with the output already bound on the nested
/// backend (`winit.rs`) and inside `offscreen::Screens::draw`; a bind
/// underneath a bind is the frozen-compositor failure above.
///
/// A warp keeps a texture, the program to draw it through, and the id and
/// commit its element carries (its pane capture's id, and a commit that moves
/// only when the capture is redrawn or the mesh's [`Shape`] changes:
/// `keyed::tests::a_warp_at_rest_keeps_its_commit`). A warped pane's popups
/// are a second list, a warp of their own drawn in front of the pane's, which
/// also keeps the part of the pane's unit square they cover
/// (`offscreen::over_job`).
///
/// Built in phases ([`effect_phases`]): in each, every capture's element list
/// first, which can run Qt, then every capture drawn on one bound carrier,
/// which must not (`offscreen::draw`); the warps last, once the chains whose
/// results their captures hold have run
/// (`tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`).
#[derive(Default)]
pub(crate) struct Prepared {
    warps: Vec<Warped>,
    overs: Vec<Warped>,
    /// Windows a refused `sol.present` geometry hides this pass
    /// (`effects.present.failed = "hide"`): nothing of them is drawn.
    /// `tests::a_refused_present_follows_its_failed_policy`.
    hidden: Vec<Window>,
    /// Every rule resolved once this pass ([`build_slots`]), so every output
    /// and every screencopy places from the same answer.
    pub(crate) slots: crate::effect::plan::Slots,
}

/// One warp `prepare` made: the capture drawn through it, the program, the id
/// and commit its element carries, the part of the pane's unit square the
/// capture covers, and the grid a geometry effect placed over that part this
/// pass (`None`: the part where it is, as a tilt draws it).
/// `warp::tests::a_lua_grid_lands_where_the_rust_genie_put_it`.
#[derive(Clone, Debug)]
pub(crate) struct Warped {
    window: Window,
    texture: GlesTexture,
    program: crate::warp::Program,
    id: Id,
    commit: CommitCounter,
    part: crate::warp::UnitRect,
    grid: Option<crate::warp::Grid>,
}

/// What a warp's mesh is a function of, in global space, so one comparison
/// serves every output and screencopy. Exact comparison: a `NaN` always
/// differs, which recommits, the safe way round.
/// `keyed::tests::a_warp_whose_mesh_moves_inside_the_same_bounds_is_given_a_new_commit`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shape {
    rect: Rectangle<f64, Logical>,
    matrix: crate::mat4::Mat4,
    pivot: (f32, f32),
    scale: f64,
    aimed: Option<present::Aimed>,
}

impl Shape {
    pub(crate) fn of(frame: &present::Frame, aimed: Option<present::Aimed>, scale: f64) -> Self {
        Self {
            rect: frame.rect,
            matrix: frame.matrix,
            pivot: frame.pivot,
            scale,
            aimed,
        }
    }
}

/// Which of a warped pane's two warps goes in first, which is nearer the
/// front: its popups, as on the flat path. `tests::a_warped_panes_popups_are_in_front_of_it`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarpPiece {
    Over,
    Pane,
}

pub(crate) const WARP_ORDER: [WarpPiece; 2] = [WarpPiece::Over, WarpPiece::Pane];

impl WarpPiece {
    /// Its place in [`WARP_ORDER`]: what a pane's per-piece state is kept
    /// by (`effect::geometry::Meshes`).
    fn index(self) -> usize {
        WARP_ORDER
            .iter()
            .position(|piece| *piece == self)
            .unwrap_or_default()
    }
}

/// The part of a pane's unit square its popups cover: their rectangle,
/// relative to the client's corner at `client_corner` in the pane, over the
/// pane's `outer` size. `tests::the_popups_part_is_their_rectangle_over_the_pane`.
pub(crate) fn over_part(
    outer: Size<i32, Logical>,
    client_corner: Point<i32, Logical>,
    covered: Rectangle<i32, Logical>,
) -> crate::warp::UnitRect {
    let (w, h) = (f64::from(outer.w.max(1)), f64::from(outer.h.max(1)));
    let (x, y) = (
        f64::from(client_corner.x + covered.loc.x),
        f64::from(client_corner.y + covered.loc.y),
    );
    crate::warp::UnitRect {
        u0: x / w,
        v0: y / h,
        u1: (x + f64::from(covered.size.w)) / w,
        v1: (y + f64::from(covered.size.h)) / h,
    }
}

/// What `prepare` does with a pane. `tests::a_window_whose_warp_has_no_program_is_drawn_flat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// Captured, and drawn through the warp.
    Warp,
    /// The flat path, its client rounded where it is if its style says so
    /// (`tests::a_style_with_a_radius_is_drawn_inline`).
    Flat,
    /// Nothing of it: a refused `sol.present` geometry under
    /// `failed = "hide"`, no capture job, no warp, no element
    /// (`tests::a_refused_present_follows_its_failed_policy`).
    Hidden,
}

/// Where a pane presented through a geometry goes: the warp, unless its mesh
/// was refused (or its folder is missing), when it follows its `failed`,
/// `effects.present.failed` or the deform's own: `Flat`, the window
/// undeformed, or `Hidden`. `tests::a_refused_present_follows_its_failed_policy`.
pub(crate) fn present_route(geometry: present::Geometry, refused: bool) -> Route {
    use crate::effect::settings::PresentFailed;
    match (refused, geometry.failed) {
        (false, _) => Route::Warp,
        (true, PresentFailed::Flat) => Route::Flat,
        (true, PresentFailed::Hide) => Route::Hidden,
    }
}

/// The version a pane's present geometry draws from: the one its id names
/// while it runs, else, under `on_reload = "keep"`, the one the present
/// pinned; `None` once its folder was reloaded under `"flat"`, which draws
/// the window flat for the rest of the transform, as an anchor that
/// resolves to nothing does.
/// `state::tests::real_client::a_reload_mid_present_follows_on_reload`.
#[cfg(test)]
pub(crate) fn present_source(
    state: &Solium,
    pane: crate::pane::PaneId,
) -> Option<std::rc::Rc<crate::effect::host::Loaded>> {
    let outer = state.pane_outer_of(pane)?;
    let geometry = state.drawn(pane, outer).deform?.effect;
    source_of(state, pane, geometry)
}

/// [`present_source`], for a geometry already in hand.
fn source_of(
    state: &Solium,
    pane: crate::pane::PaneId,
    geometry: present::Geometry,
) -> Option<std::rc::Rc<crate::effect::host::Loaded>> {
    use crate::effect::settings::PresentReload;
    state.effects.by_id(geometry.effect).or_else(|| {
        (geometry.on_reload == PresentReload::Keep)
            .then(|| state.panes.get(pane)?.meshes().pinned.clone())
            .flatten()
    })
}

/// What `prepare` does with a pane this pass, before anything is captured.
/// `state::tests::real_client::a_present_deform_is_a_file`.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "one a pane a pass, matched at once and never stored"
)]
pub(crate) enum Planned {
    /// Nothing of it: a refused present geometry under `failed = "hide"`.
    Hidden,
    /// The flat path: not warped, or a present geometry that is refused, or
    /// whose folder was reloaded, on a frame with no matrix.
    Flat,
    /// Warped: its frame, what its deform is aimed at, and the grid a
    /// present geometry placed (`None` for a tilt alone, or a refused
    /// geometry on a tilted frame, drawn undeformed).
    Warp {
        frame: present::Frame,
        aimed: Option<present::Aimed>,
        grid: Option<crate::warp::Grid>,
    },
}

/// [`warp_of`], and a present geometry's grid built once a pass: refused,
/// the window follows its `failed` (undeformed, as an unresolved anchor is,
/// still tilted if it is; or nothing of it), and with its folder reloaded
/// under `on_reload = "flat"` it is undeformed. Asked with no renderer.
/// `tests::a_refused_present_follows_its_failed_policy`,
/// `state::tests::real_client::a_present_deform_is_a_file`,
/// `state::tests::real_client::effects_present_is_the_default_a_deform_overrides`.
pub(crate) fn plan_warp(
    state: &mut Solium,
    pane: crate::pane::PaneId,
    outer: Rectangle<i32, Logical>,
    scale: f64,
) -> Planned {
    let Some((frame, aimed)) = warp_of(state, pane, outer) else {
        return Planned::Flat;
    };
    let Some(aimed) = aimed else {
        return Planned::Warp {
            frame,
            aimed: None,
            grid: None,
        };
    };
    let gridded = present_grid(
        state,
        pane,
        &frame,
        aimed,
        (WarpPiece::Pane, crate::warp::UnitRect::WHOLE),
        scale,
    );
    let route = match gridded {
        Gridded::Grid(grid) => {
            return Planned::Warp {
                frame,
                aimed: Some(aimed),
                grid: Some(grid),
            };
        }
        Gridded::Refused => present_route(aimed.effect, true),
        Gridded::Undeformed => Route::Flat,
    };
    match route {
        Route::Hidden => Planned::Hidden,
        _ if frame.matrix.is_identity() => Planned::Flat,
        _ => Planned::Warp {
            frame,
            aimed: None,
            grid: None,
        },
    }
}

/// What a present geometry's grid came to this pass.
#[derive(Debug)]
enum Gridded {
    /// Its grid, over the part asked for.
    Grid(crate::warp::Grid),
    /// No source: its folder was reloaded mid-flight under `on_reload =
    /// "flat"`. The window undeformed.
    Undeformed,
    /// Refused, or its folder missing: what its `failed` says.
    Refused,
}

/// A present geometry's grid for a piece of the pane over its `part`, built
/// once a pass (not once per output) in global logical pixels through the
/// effect's `mesh`, and kept on the pane while nothing it depends on moves
/// (`effect::geometry::tests::a_mesh_is_not_rebuilt_when_neither_progress_nor_an_anchor_changed`).
/// A refusal is logged once a piece and counted in the trace each pass
/// (`state::tests::real_client::a_present_refused_for_its_popups_alone_is_said_once`). In a debug build,
/// when `SOLIUM_GEOMETRY_ORACLE` names the effect, the grid is the Rust
/// genie's instead, the oracle `dev/effects-check.sh genie` compares the
/// folder with; a release build has neither the branch nor the oracle.
fn present_grid(
    state: &mut Solium,
    pane: crate::pane::PaneId,
    frame: &present::Frame,
    aimed: present::Aimed,
    (piece, part): (WarpPiece, crate::warp::UnitRect),
    scale: f64,
) -> Gridded {
    use crate::effect::geometry::{Ask, MeshKey, mesh, turned, unpacked};
    let geometry = aimed.effect;
    if geometry.effect == crate::effect::host::EffectId::NONE {
        return Gridded::Refused;
    }
    let Some(loaded) = source_of(state, pane, geometry) else {
        return Gridded::Undeformed;
    };
    let (from, to) = (
        present::for_effects(frame.rect),
        present::for_effects(aimed.to),
    );
    #[cfg(debug_assertions)]
    if crate::dev::geometry_oracle().as_deref() == Some(loaded.name()) {
        let spread = unpacked(loaded.defaults(), &geometry.params)
            .into_iter()
            .find_map(|(name, value)| match (name.as_str(), value) {
                ("spread", solium_effects::spec::Value::Number(spread)) => Some(spread),
                _ => None,
            })
            .unwrap_or(1.0);
        #[expect(clippy::cast_possible_truncation, reason = "the Rust genie's own f32s")]
        let deform = solium_effects::Deform::Genie {
            progress: geometry.progress as f32,
            spread: spread as f32,
            axis: geometry.axis,
        };
        return Gridded::Grid(crate::warp::oracle_grid(frame.rect, aimed.to, deform, part));
    }
    let monitor = state
        .pane_outer_of(pane)
        .and_then(|slot| state.output_of(slot))
        .and_then(|output| state.space.output_geometry(&output))
        .map_or(from, |area| present::for_effects(area.to_f64()));
    let (cols, rows) = turned(
        loaded
            .spec()
            .grid
            .unwrap_or(solium_effects::spec::GridSpec::Fixed { cols: 1, rows: 1 }),
        geometry.axis,
    );
    // A `sol.present` geometry measures how far the window is pulled into
    // its target: a leaving's progress, so `t.direction` is -1.
    let ask = Ask {
        progress: geometry.progress,
        clamped: geometry.progress.clamp(0.0, 1.0),
        direction: -1.0,
        axis: geometry.axis,
        from,
        to: Some(to),
        part,
        seed: geometry.seed,
        monitor,
        scale,
        params: &[],
    };
    let key = MeshKey::new(geometry.effect, geometry.params, &ask, cols, rows);
    let Some(held) = state.panes.get_mut(pane) else {
        return Gridded::Undeformed;
    };
    let meshes = held.meshes_mut();
    let built = meshes
        .get_or_build(key, || {
            let params = unpacked(loaded.defaults(), &geometry.params);
            let ask = Ask {
                params: &params,
                ..ask
            };
            mesh(loaded.sandbox(), &ask, cols, rows).map(|points| crate::warp::Grid {
                cols,
                rows,
                part,
                points,
            })
        })
        .cloned();
    match built {
        Ok(grid) => {
            meshes.built(piece.index());
            Gridded::Grid(grid)
        }
        Err(refusal) => {
            crate::pacing::mesh_refused();
            if meshes.say_refused(piece.index()) {
                tracing::warn!(
                    effect = loaded.name(),
                    ?refusal,
                    "a present's mesh was refused; the window is drawn by its `failed`"
                );
            }
            Gridded::Refused
        }
    }
}

/// What a warped pane's popups are drawn through under a present geometry:
/// their own grid, over their `part`; refused, what the geometry's `failed`
/// says, the identity grid of their part (`Some(None)`, drawn undeformed)
/// under `"flat"` and nothing of them (`None`) under `"hide"`; and drawn
/// undeformed once the folder was reloaded under `on_reload = "flat"`, as
/// the pane is.
/// `state::tests::real_client::a_present_refused_for_its_popups_alone_is_said_once`.
pub(crate) fn over_grid(
    state: &mut Solium,
    pane: crate::pane::PaneId,
    frame: &present::Frame,
    aimed: present::Aimed,
    part: crate::warp::UnitRect,
    scale: f64,
) -> Option<Option<crate::warp::Grid>> {
    match present_grid(state, pane, frame, aimed, (WarpPiece::Over, part), scale) {
        Gridded::Grid(grid) => Some(Some(grid)),
        Gridded::Undeformed => Some(None),
        Gridded::Refused => match present_route(aimed.effect, true) {
            Route::Hidden => None,
            _ => Some(None),
        },
    }
}

/// Warp a pane only when it is deformed **and** there is a program to warp it
/// with; otherwise it takes the flat path, where a missing warp costs a tilt
/// and never the window.
pub(crate) fn route(warped: bool, program: bool) -> Route {
    if warped && program {
        Route::Warp
    } else {
        Route::Flat
    }
}

/// `prepare`'s warp condition for a pane drawn over `outer`: the frame it is
/// drawn with this pass and what its deform is aimed at, when its matrix is
/// not the identity or its deform is aimed at something; `None` when it is
/// drawn flat, which with no effects is every window at rest, a fullscreen
/// one included (Ruling 24).
///
/// The anchor is resolved here, and not merely tested for presence: a deform
/// aimed at a pane that has closed draws flat, and capturing a texture for it
/// would be a megabyte a frame spent on a warp that `elements` has already
/// decided not to do.
/// `state::tests::real_client::a_fullscreen_window_with_no_rule_takes_todays_path`.
pub(crate) fn warp_of(
    state: &Solium,
    pane: crate::pane::PaneId,
    outer: Rectangle<i32, Logical>,
) -> Option<(present::Frame, Option<present::Aimed>)> {
    let frame = state.drawn(pane, outer);
    let aimed = state.aimed_at_for(pane, frame.deform);
    (!frame.matrix.is_identity() || aimed.is_some()).then_some((frame, aimed))
}

/// Whether `prepare` warps a pane this pass, asked with no renderer: its
/// client's outer rectangle, as `prepare` finds it, through [`warp_of`].
/// `state::tests::real_client::a_fullscreen_window_with_no_rule_takes_todays_path`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "prepare asks warp_of with the rectangle it has")
)]
pub(crate) fn wants_warp(state: &Solium, pane: crate::pane::PaneId) -> bool {
    state
        .panes
        .get(pane)
        .and_then(crate::pane::Pane::client)
        .and_then(|window| state.outer_geometry(window))
        .is_some_and(|outer| warp_of(state, pane, outer).is_some())
}

impl Prepared {
    /// Lend the warp captured for `window`: the texture to draw through it,
    /// the program, the id and commit it carries, and its grid.
    ///
    /// Lent rather than taken: with more than one monitor `elements` runs once
    /// per output, and a texture removed by the first one would leave a
    /// deformed window undrawn on every other screen. `GlesTexture` is a
    /// handle, so the clone is a refcount, and the program is GL names.
    fn warp(&self, window: &Window) -> Option<&Warped> {
        self.warps.iter().find(|each| each.window == *window)
    }

    /// Lend the capture of `window`'s popups, for the warp drawn in front of
    /// its own, with the part of the pane's unit square it covers. Lent for
    /// `warp`'s reason. `None` when it has no popups open.
    /// `dev/present-check.sh`'s `menu` case draws it.
    fn over(&self, window: &Window) -> Option<&Warped> {
        self.overs.iter().find(|each| each.window == *window)
    }

    /// Whether nothing of `window` is drawn this pass: a refused present
    /// geometry under `failed = "hide"`.
    /// `tests::a_refused_present_follows_its_failed_policy`.
    fn hidden(&self, window: &Window) -> bool {
        self.hidden.contains(window)
    }
}

/// The probe's answer: set on the host, and a rebind after the frame when it
/// changed what was known, since rules bound before it kept every rung
/// (Ruling 11). `state::tests::the_formats_probe_rebinds_the_rules_after_the_frame`.
pub(crate) fn note_formats(state: &mut Solium, found: crate::pool::Formats) {
    if state.effects.set_formats(found) {
        state.rebind = true;
    }
}

/// Capture a texture for every window that cannot be drawn from its surfaces
/// where they are: one whose transform is not a rectangle. A rounded window is
/// drawn from its surfaces, each through the clipped programs this compiles
/// (`tests::a_style_with_a_radius_is_drawn_inline`).
///
/// Must run before the backend binds its own buffer; see [`Prepared`].
/// Note on the clock: this walk calls [`Solium::drawn`], which samples the
/// clock per pane — the very thing that function's own doc now tells a
/// multi-pane caller not to do. It is left as it was, deliberately: `prepare`
/// decides what to **capture**, and a capture is keyed on a window and a size,
/// neither of which a few microseconds of skew can change. `elements` was the
/// caller where the skew was visible, and that is the one that was fixed. A
/// drive-by change here would be churn.
pub(crate) fn prepare(state: &mut Solium, renderer: &mut GlesRenderer) -> Prepared {
    // Everything here is the compositor's own work, ahead of any output, apart
    // from the tick below — which is entirely Qt's and is measured separately
    // for exactly that reason. See `pacing::Phase`.
    let _prep = crate::pacing::span(crate::pacing::Phase::Prep);
    state.memory_report();
    // What the pointer is standing on can change without the pointer moving --
    // a window slides under it, a mode opens -- and there is no input event for
    // that. Asked here rather than in `Solium::settle`, which runs *after* the
    // frame it settles: a titlebar arriving under a still pointer was drawn
    // once with the shape it had a moment ago and corrected only on the frame
    // the self-inflicted damage bought. This is before any cursor element is
    // built, so the shape this finds is the shape this frame draws.
    state.reassert_cursor();
    // Every QML animation in the process, advanced once for this frame --
    // decorations, the cursor, the shell. Whether any scene then has something
    // new to draw is each scene's own answer.
    //
    // Once per *frame* and not once per output: with two monitors, ticking in
    // `elements` would advance every animation twice as fast as the clock, and
    // on monitors of different refresh rates by different amounts.
    {
        let _tick = crate::pacing::span(crate::pacing::Phase::Tick);
        // The models hosted scenes read, one batch each, ahead of the tick
        // and measured with it as Qt's, because applying a row runs every
        // binding and handler on it:
        // `models::tests::publish_models_carries_the_compositors_monitors_to_their_scenes`,
        // `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`.
        state.publish_models();
        crate::qml::tick(state.clock.now());
    }

    // Twice the largest monitor's bytes may wait on the pool's free list
    // (Ruling 10): `pool::tests::a_target_given_back_over_budget_is_dropped`.
    let largest = state
        .space
        .outputs()
        .filter_map(|output| output.current_mode().map(|mode| mode.size))
        .map(|size| usize::try_from(size.w).unwrap_or(0) * usize::try_from(size.h).unwrap_or(0) * 4)
        .max()
        .unwrap_or(0);
    state.pool.set_budget(2 * largest);

    // The effects and the captures, in the one order that lets a warped
    // pane's capture hold its slots' results (Ruling 17):
    // `tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`.
    let mut gpu = Gpu {
        state,
        renderer,
        drawn: crate::effect::store::Drawn::default(),
        warps: Vec::new(),
        overs: Vec::new(),
        hidden: Vec::new(),
        pass: 0,
    };
    let slots = effect_phases(&mut gpu);
    Prepared {
        warps: gpu.warps,
        overs: gpu.overs,
        hidden: gpu.hidden,
        slots,
    }
}

/// `prepare`'s effect phases, so their order is one function a test drives
/// (`tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`).
pub(crate) trait Phases {
    /// Compile what the host was asked for, probe the formats, compile
    /// `Programs::masked` when an effect is wanted (Ruling 7, spec C14).
    fn compile(&mut self);
    /// Resolve the slots once this pass ([`build_slots`] over the panes a
    /// monitor shows) and record their boxes ([`record_boxes`]).
    fn resolve(&mut self) -> Slots;
    /// Build and draw one nest's self captures; a capture that is not stale
    /// is kept. Both go into the pass's `Drawn`.
    fn captures(&mut self, slots: &Slots, nest: Nest);
    /// Run one nest's chains on one bound carrier ([`run_slots`]).
    fn chains(&mut self, slots: &mut Slots, nest: Nest);
    /// Build the warp and `over` jobs from `slots` as they are now, draw
    /// them, and place what they made in `Prepared`.
    fn warps(&mut self, slots: &Slots);
    /// The store's sweep and the pool's, once a pass.
    fn sweep(&mut self);
}

/// `prepare`'s order (Ruling 17): the warp and `over` jobs are built only
/// once every chain that can put a result in them has run this pass, and a
/// whole pane's self capture only once the chains inside it have.
/// `tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`.
pub(crate) fn effect_phases(phases: &mut impl Phases) -> Slots {
    phases.compile();
    let mut slots = phases.resolve();
    for nest in [Nest::Inner, Nest::Whole] {
        phases.captures(&slots, nest);
        phases.chains(&mut slots, nest);
    }
    phases.warps(&slots);
    phases.sweep();
    slots
}

/// [`Phases`] on the GPU: `prepare`'s own work, after the memory report,
/// the QML tick and the pool's budget, driven in the order
/// `tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`
/// pins. What the passes make is gathered here for `Prepared`.
struct Gpu<'a> {
    state: &'a mut Solium,
    renderer: &'a mut GlesRenderer,
    /// Each self input this pass, kept or drawn: what the chains read
    /// (`effect::store::tests::drawn_tells_an_input_redrawn_this_pass_from_one_kept`).
    drawn: crate::effect::store::Drawn,
    warps: Vec<Warped>,
    overs: Vec<Warped>,
    hidden: Vec<Window>,
    /// The store's pass, set by `resolve`.
    pass: u64,
}

impl std::fmt::Debug for Gpu<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gpu")
            .field("warps", &self.warps.len())
            .field("overs", &self.overs.len())
            .field("hidden", &self.hidden.len())
            .field("pass", &self.pass)
            .finish_non_exhaustive()
    }
}

impl Phases for Gpu<'_> {
    fn compile(&mut self) {
        let (state, renderer) = (&mut *self.state, &mut *self.renderer);
        // Effects compile here, between frames, where the context is current:
        // nothing at all when no effect is wanted (spec §8.4,
        // `effect::host::tests::an_empty_host_touches_no_gl`). The formats a
        // plan may draw into are probed first, once, and only once something is
        // wanted (`effect::host::tests::the_formats_are_probed_once_and_only_while_something_is_wanted`;
        // the probe itself is wirecheck case 12c's). A changed answer rebinds
        // the rules after the frame (`note_formats`).
        if state.effects.is_idle() {
            return;
        }
        if state.effects.wants_formats() {
            let found = crate::pool::probe_formats(renderer);
            note_formats(state, found);
        }
        state
            .effects
            .compile_pending(&mut crate::effect::GlCompiler(renderer));
        // What a slot's result is drawn through, compiled here, between
        // frames, while an effect is wanted, since `elements` never compiles;
        // nothing with none (spec §8.4,
        // `effect::host::tests::an_empty_host_touches_no_gl`).
        let _ = state.programs.masked(renderer);
    }

    fn resolve(&mut self) -> Slots {
        let state = &mut *self.state;
        // Every rule resolved once, after the effects compiled and before any
        // job is built; nothing, and no fact gathered, with no rules (spec §8.4,
        // `state::tests::real_client::with_no_rules_no_slot_is_wanted_and_no_fact_is_gathered`).
        let mut slots = build_slots(state);
        // Every wanted slot's box, and its state in the store, kept while it is
        // wanted and given back by the sweep once it is not
        // (`effect::store::tests::a_slot_no_rule_wanted_this_pass_is_dropped`).
        record_boxes(state, &mut slots);
        self.pass = state.store.next_pass();
        for (owner, slot, key) in slots.wants() {
            state.store.slot_mut(owner, slot, key).seen = self.pass;
        }
        slots
    }

    fn captures(&mut self, slots: &Slots, nest: Nest) {
        let (state, renderer) = (&mut *self.state, &mut *self.renderer);
        let mut jobs = Vec::new();
        // A part's self input, padded by its chain's reach and drawn only when
        // what it is drawn from differs (`offscreen::kept`,
        // `state::tests::real_client::a_self_rule_captures_the_client_once_until_it_commits`).
        // A whole pane's is built in the later nest, so it walks what the
        // inner chains made this pass
        // (`tests::a_whole_panes_capture_walks_its_inner_slots_and_not_its_own`).
        // None with no slot wanted, and then no carrier is bound (spec §8.4).
        for input in self_inputs(state, slots) {
            if Nest::of(&input.owner, crate::effect::rules::Tier::Own) != nest {
                continue;
            }
            let Some(source) = input.found.source() else {
                continue;
            };
            let Some(job) = crate::offscreen::part_job(
                state,
                renderer,
                input.owner.clone(),
                input.slot,
                source,
                input.scale,
                input.pad,
                slots,
            ) else {
                continue;
            };
            match crate::offscreen::kept(state, &job) {
                Some((texture, _id, commit)) => {
                    self.drawn
                        .insert(input.owner, input.slot, texture, false, commit);
                }
                None => jobs.push((job, (input.owner, input.slot))),
            }
        }
        for ((owner, slot), texture, _id, commit) in crate::offscreen::draw(state, renderer, jobs) {
            self.drawn.insert(owner, slot, texture, true, commit);
        }
    }

    fn chains(&mut self, slots: &mut Slots, nest: Nest) {
        // The chains, once their inputs are drawn or kept, on one bound
        // carrier (Ruling 10): a self chain runs only when its part committed
        // or its params or size changed, and is otherwise placed again as it
        // was (`effect::store::tests::a_self_chain_runs_once_until_its_part_commits`).
        // Nothing runs and no carrier is bound with no chain of this nest
        // wanted (spec §8.4,
        // `tests::a_run_phase_with_no_chain_of_its_own_binds_nothing`).
        if !runs_in(slots, &self.state.chains, nest) {
            return;
        }
        let Self {
            state,
            renderer,
            drawn,
            pass,
            ..
        } = self;
        let Solium {
            store,
            pool,
            effects,
            chains,
            programs,
            timer,
            clock,
            ..
        } = &mut **state;
        let mut cx = RunCx {
            store,
            effects: &*effects,
            chains,
            masked: programs.masked_compiled().cloned(),
            now: clock.now().as_secs_f32(),
            pass: *pass,
        };
        let drawn = &*drawn;
        let _ = with_carrier(pool, renderer, |renderer, carrier, pool| {
            let mut runner = GlRunner::new(renderer, carrier, timer.as_mut());
            run_slots(&mut cx, pool, &mut runner, slots, drawn, nest);
        });
    }

    fn warps(&mut self, slots: &Slots) {
        let (state, renderer) = (&mut *self.state, &mut *self.renderer);
        let mut jobs = Vec::new();
        for (pane, window) in state.on_screen() {
            // Nothing captured means nothing to keep. A pane holds the targets it
            // was last captured into between frames -- megabytes of them -- and
            // there is no later frame on which handing them back gets cheaper, so
            // an overview that warps twenty windows and is then closed would
            // otherwise leave twenty behind for the session. See
            // `keyed::Captures`, and
            // `keyed::tests::a_pane_that_stops_warping_gives_the_texture_back`.
            let release = |state: &mut Solium| crate::offscreen::release(state, pane);
            let Some(window) = window else {
                release(state);
                continue;
            };
            let Some(outer) = state.outer_geometry(&window) else {
                release(state);
                continue;
            };
            // A rounded client's programs, compiled here, between frames, so the
            // first frame that draws it has them: `elements` never compiles
            // (`clipped`). Before the cull and the guard below, where a rounded
            // pane now stops, wanting no capture
            // (`tests::a_pane_neither_warped_nor_styled_wants_no_capture`); the
            // rounded shot of `dev/pacing-nested.sh` draws through them.
            if declared_rounding(state, pane).is_some() {
                let _ = state.programs.clip(renderer);
            }
            // **A pane no monitor shows is not captured**, and this is the same
            // question `elements` asks one screen at a time before it draws
            // anything: `!global.overlaps(screen)`, the containment rule that
            // makes a workspace switch work. A hidden workspace is not unmapped,
            // it is *parked a screen away*, so without this every window on every
            // workspace is captured on every frame for as long as the session
            // lasts -- and `release` below never fires either, because the capture
            // keeps succeeding.
            //
            // It was survivable while a capture meant a warp: a window is deformed
            // for the length of an animation and then stops. A `client.radius` is
            // permanent, so twelve windows across three workspaces became twelve
            // full-window offscreen renders a frame and ~47 MB of captures held
            // for the session, a third of it for windows nothing ever draws.
            //
            // **The slot, through the same `pane_outer_of` call `elements` makes,
            // and not the `outer` above.** They differ in exactly two places and
            // the slot is the safer of the two in both: `pane_geometry` falls back
            // to the layout's rectangle for a window that has mapped without a
            // size yet -- where `outer_geometry` is a zero-size rect that overlaps
            // nothing and would be culled while `elements` went on to draw it --
            // and `insets_of` grows by the decoration's insets where
            // `frame_insets` gives an undecorated window none. So the question is
            // asked of the rectangle `elements` will ask it of.
            //
            // Being wrong in that direction is *not* a blank window: a pane with
            // no capture is drawn from its surfaces by the flat path, rounded or
            // not, so a wrongly culled warp is a FLAT window for one frame. Worth
            // knowing, because it sets how hard to lean -- the failure is cosmetic
            // and self-correcting, while being wrong the other way is a capture
            // per window per frame for the life of the session.
            //
            // Costed honestly: `pane_outer_of` is two linear `Panes::get` scans
            // (one through `insets_of`) plus an `element_location`, so this is
            // O(panes) per pane per frame, not the single `overlaps` it reads as.
            // About three hundred comparisons at twelve panes -- nothing beside an
            // offscreen render, and the reason the cheap case stays cheap is that
            // `on_screen` is short, not that this line is.
            if !shown(state, pane) {
                release(state);
                continue;
            }
            // At *its own monitor's* scale. One frame can span monitors at
            // different scales, and a texture taken at 1x and drawn on a 2x screen
            // is the blur this whole change exists to remove.
            //
            // A deformed window with no warp program takes the flat path below
            // instead of being captured for a warp that cannot be drawn:
            // `tests::a_window_whose_warp_has_no_program_is_drawn_flat`.
            let scale = state.scale_of(outer);
            let (warp, grid) = match plan_warp(state, pane, outer, scale) {
                Planned::Hidden => {
                    self.hidden.push(window);
                    release(state);
                    continue;
                }
                Planned::Flat => (None, None),
                Planned::Warp { frame, aimed, grid } => (Some((frame, aimed)), grid),
            };
            let program = if warp.is_some() {
                state.programs.warp(renderer)
            } else {
                None
            };
            let routed = route(warp.is_some(), program.is_some());
            // A pane that is not warped builds no job, rounded or not (spec §8.4).
            // `tests::a_pane_neither_warped_nor_styled_wants_no_capture`.
            if wanted_capture(routed).is_none() {
                // Said out loud rather than skipped. Not drawing `Inputs::Backdrop`
                // is right -- there is nothing composited beneath a node for this
                // renderer to sample -- but a blur that silently renders as no
                // blur looks like a style that failed to load and is never
                // reported as a compositor bug. See `fragment::Inputs::Backdrop`,
                // and
                // `pass::tests::an_effect_that_cannot_be_run_is_named_once_and_not_every_frame`.
                if let Some(refused) = crate::pass::refused(&declared_effects(state, pane)) {
                    state.programs.refuse(refused);
                }
                release(state);
                continue;
            }
            if let (Route::Warp, Some(program), Some((frame, aimed))) = (routed, program, warp) {
                // What its mesh is drawn from, in global space: the warp's commit
                // moves when this does, as well as when its capture is redrawn.
                // `keyed::tests::a_warp_whose_mesh_moves_inside_the_same_bounds_is_given_a_new_commit`.
                let shape = Shape::of(&frame, aimed, scale);
                let job = crate::offscreen::pane_job(state, renderer, pane, &window, scale, slots);
                // Its popups the same way, in a capture of their own that `panes`
                // draws in front of the pane's warp
                // (`tests::a_warped_panes_popups_are_in_front_of_it`), kept until
                // they commit:
                // `state::tests::real_client::a_commit_on_a_popup_makes_the_popups_capture_stale`.
                // Under a present geometry their part has a grid of its own,
                // and a grid refused follows the geometry's `failed`: the
                // popups undeformed, or none of them, the pane's warp kept.
                let over = if job.is_some() {
                    crate::offscreen::over_job(state, renderer, pane, &window, scale)
                } else {
                    None
                };
                let over = match (over, aimed) {
                    (Some((job, part)), Some(aimed)) => {
                        over_grid(state, pane, &frame, aimed, part, scale)
                            .map(|grid| (job, part, grid))
                    }
                    (over, _) => over.map(|(job, part)| (job, part, None)),
                };
                // Holding only what it captures this pass: popups that closed give
                // their capture back while the pane goes on warping.
                // `keyed::tests::a_warped_pane_whose_popups_close_gives_their_capture_back`.
                let kinds: &[crate::keyed::Kind] = if over.is_some() {
                    &[crate::keyed::Kind::Pane, crate::keyed::Kind::Over]
                } else {
                    &[crate::keyed::Kind::Pane]
                };
                let (panes, pool) = (&mut state.panes, &mut state.pool);
                if let Some(held) = panes.get_mut(pane) {
                    held.captures_mut().keep(kinds, pool);
                }
                if let Some(job) = job {
                    // Already drawn from exactly this: no frame (`offscreen::kept`,
                    // `state::tests::real_client::a_capture_whose_surface_tree_has_not_committed_is_not_drawn_again`).
                    if let Some((texture, id, _commit)) = crate::offscreen::kept(state, &job) {
                        let commit =
                            warp_commit(state, pane, crate::keyed::Kind::Pane, shape, false);
                        self.warps.push(Warped {
                            window: window.clone(),
                            texture,
                            program,
                            id,
                            commit,
                            part: crate::warp::UnitRect::WHOLE,
                            grid,
                        });
                    } else {
                        jobs.push((job, Then::Warp(window.clone(), program, pane, shape, grid)));
                    }
                }
                if let Some((job, part, grid)) = over {
                    if let Some((texture, id, _commit)) = crate::offscreen::kept(state, &job) {
                        let commit =
                            warp_commit(state, pane, crate::keyed::Kind::Over, shape, false);
                        self.overs.push(Warped {
                            window,
                            texture,
                            program,
                            id,
                            commit,
                            part,
                            grid,
                        });
                    } else {
                        jobs.push((job, Then::Over(window, program, pane, shape, part, grid)));
                    }
                }
                continue;
            }
            release(state);
        }

        // Every list is built, from the slots as the chains left them: draw
        // them all, on one carrier, and place what they made
        // (`tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`).
        for (then, texture, id, _commit) in crate::offscreen::draw(state, renderer, jobs) {
            match then {
                Then::Warp(window, program, pane, shape, grid) => {
                    let commit = warp_commit(state, pane, crate::keyed::Kind::Pane, shape, true);
                    self.warps.push(Warped {
                        window,
                        texture,
                        program,
                        id,
                        commit,
                        part: crate::warp::UnitRect::WHOLE,
                        grid,
                    });
                }
                Then::Over(window, program, pane, shape, part, grid) => {
                    let commit = warp_commit(state, pane, crate::keyed::Kind::Over, shape, true);
                    self.overs.push(Warped {
                        window,
                        texture,
                        program,
                        id,
                        commit,
                        part,
                        grid,
                    });
                }
            }
        }
    }

    fn sweep(&mut self) {
        // A slot no rule wanted this pass gives its targets back, and the
        // pool deletes what was given back, once a pass and not in each
        // draw (Ruling 17).
        // `effect::store::tests::a_slot_no_rule_wanted_this_pass_is_dropped`.
        let state = &mut *self.state;
        state.store.sweep(self.pass, &mut state.pool);
        state.pool.sweep(self.renderer);
    }
}

/// Whether any monitor shows a pane, asked of the rectangle `elements` asks
/// it of: `prepare`'s cull, which [`build_slots`] makes too. `prepare`'s walk
/// says at length why the slot and not the client's rectangle.
/// `state::tests::real_client::a_pane_no_monitor_shows_gathers_no_fact_and_wants_no_slot`.
fn shown(state: &Solium, pane: crate::pane::PaneId) -> bool {
    state
        .pane_outer_of(pane)
        .is_some_and(|slot| state.on_any_output(slot))
}

/// What a capture becomes once drawn: `dev/fence-check.sh` has two warps,
/// `dev/present-check.sh`'s menu case a warp and its popups. A warp
/// carries its pane and the [`Shape`] its commit is moved by; its popups' warp
/// the part of the pane they cover as well.
#[derive(Debug)]
enum Then {
    /// The pane's own warp, and its grid.
    Warp(
        Window,
        crate::warp::Program,
        crate::pane::PaneId,
        Shape,
        Option<crate::warp::Grid>,
    ),
    /// Its popups', over their part, and its grid.
    Over(
        Window,
        crate::warp::Program,
        crate::pane::PaneId,
        Shape,
        crate::warp::UnitRect,
        Option<crate::warp::Grid>,
    ),
}

/// The commit `pane`'s `kind` of warp carries this pass, its own or its
/// popups': moved when its capture was `redrawn` or its `shape` changed, and
/// only then. `keyed::tests::a_warp_at_rest_keeps_its_commit`,
/// `keyed::tests::the_popups_warp_commits_apart_from_the_panes`.
fn warp_commit(
    state: &mut Solium,
    pane: crate::pane::PaneId,
    kind: crate::keyed::Kind,
    shape: Shape,
    redrawn: bool,
) -> CommitCounter {
    state
        .panes
        .get_mut(pane)
        .map(|held| held.captures_mut().warp_commit_for(kind, shape, redrawn))
        .unwrap_or_default()
}

/// The effects a pane's style declares, copied out so nothing borrows the
/// state: nought or one, and `Vec::new()` (no allocation) for every
/// unstyled window. Lifted from what was `client_pass`.
/// `decoration::tests::a_decoration_with_no_declared_radius_runs_no_pass`.
fn declared_effects(
    state: &Solium,
    pane: crate::pane::PaneId,
) -> Vec<solium_effects::fragment::Effect> {
    state
        .panes
        .get(pane)
        .and_then(Pane::decoration)
        .map(crate::decoration::Decoration::effects)
        .unwrap_or_default()
        .to_vec()
}

/// A pane's style rules and the generation they were read at: its
/// decoration's for a framed pane, and none for a bare one (fullscreen, CSD,
/// `pane = "none"`), so a style's rules never reach a pane it does not frame
/// (Ruling 15). The one place `prepare` reads them from.
/// `tests::a_bare_pane_gets_no_style_rules`,
/// `decoration::tests::a_decoration_carries_the_styles_rules`.
pub(crate) fn style_rules(frame: &crate::pane::Frame) -> (&[crate::effect::rules::Rule], u32) {
    match frame {
        crate::pane::Frame::Styled(decoration) => {
            (decoration.rules(), decoration.rules_generation())
        }
        crate::pane::Frame::Pending | crate::pane::Frame::None => (&[], 0),
    }
}

/// Resolve every rule once this pass: for every part of every pane a monitor
/// shows (the cull `prepare` makes), every scripted surface's instance on
/// each output, and every client layer surface, what each slot wants. A
/// resolved key wants a slot only when its chain is bound: every chain was
/// bound at config load or when its style was applied (Ruling 15), so this
/// binds nothing, and a style's rule that could not bind, already on the
/// overlay, wants none. With no rules, the user's or any pane's style's,
/// nothing is resolved and no fact gathered (spec §8.4).
/// `state::tests::real_client::with_no_rules_no_slot_is_wanted_and_no_fact_is_gathered`,
/// `state::tests::real_client::a_rule_on_focused_follows_the_keyboard`,
/// `state::tests::real_client::a_surface_has_a_slot_per_monitor_and_a_layer_surface_one_of_its_own`.
pub(crate) fn build_slots(state: &mut Solium) -> crate::effect::plan::Slots {
    use crate::effect::plan::{self, PaneSlot, Slots};
    use crate::effect::rules::{Facts, PartRef};
    use smithay::reexports::wayland_server::Resource as _;
    let state: &Solium = state;
    let mut slots = Slots::default();
    if state.rules.is_empty()
        && state
            .panes
            .iter()
            .all(|pane| style_rules(pane.frame()).0.is_empty())
    {
        return slots;
    }
    for held in state.panes.iter() {
        let pane = held.id();
        if !shown(state, pane) {
            continue;
        }
        let (style, generation) = style_rules(held.frame());
        let facts = state.window_facts(pane, state.rules.uses(style));
        slots.gathered();
        let view = Facts {
            app_id: &facts.app_id,
            title: &facts.title,
            focused: facts.focused,
            fullscreen: facts.fullscreen,
            monitor: &facts.monitor,
            style: &facts.style,
            ..Facts::default()
        };
        let mut want = |slot_of: PaneSlot, part: PartRef<'_>| {
            want_resolved(
                &mut slots,
                state,
                &plan::Owner::Pane(pane, slot_of),
                state.rules.resolve(style, generation, part, &view),
            );
        };
        want(PaneSlot::Pane, PartRef::Pane);
        want(PaneSlot::Client, PartRef::Client);
        want(PaneSlot::Popups, PartRef::Popup);
        want(PaneSlot::Titlebar, PartRef::Region("titlebar"));
        for (index, name) in held
            .decoration()
            .into_iter()
            .flat_map(|decoration| decoration.layer_names())
        {
            want(PaneSlot::Layer(index), PartRef::Layer(name));
        }
    }
    // A style's rules are for its panes only (Ruling 15): surfaces and layer
    // surfaces resolve the user's, so with none there is nothing to resolve.
    if state.rules.is_empty() {
        return slots;
    }
    let primary = state.primary_output();
    for output in state.space.outputs() {
        // One owner per scripted surface's instance on each output it is on,
        // where `elements` puts it (`wanted`), matched by its name.
        if let Some(geometry) = state.space.output_geometry(output) {
            for surface in state.surfaces.iter() {
                if surface
                    .area_on(output, geometry, primary.as_ref())
                    .is_none()
                {
                    continue;
                }
                slots.gathered();
                let name = surface.name();
                let facts = Facts {
                    surface: name,
                    ..Facts::default()
                };
                let resolved = state.rules.resolve(&[], 0, PartRef::Surface(name), &facts);
                if !resolved.is_empty() {
                    want_resolved(
                        &mut slots,
                        state,
                        &plan::Owner::Surface(surface.id(), output.name()),
                        resolved,
                    );
                }
            }
        }
        // And one per client layer surface, matched by its namespace.
        let map = layer_map_for_output(output);
        for layer in map.layers() {
            slots.gathered();
            let namespace = layer.namespace();
            let facts = Facts {
                layer_shell: namespace,
                ..Facts::default()
            };
            let resolved = state
                .rules
                .resolve(&[], 0, PartRef::LayerShell(namespace), &facts);
            if !resolved.is_empty() {
                want_resolved(
                    &mut slots,
                    state,
                    &plan::Owner::LayerShell(layer.wl_surface().id()),
                    resolved,
                );
            }
        }
    }
    slots
}

/// Want each slot a part resolved to whose chain is bound.
/// `state::tests::real_client::a_rule_whose_chain_is_not_bound_wants_no_slot`.
fn want_resolved(
    slots: &mut crate::effect::plan::Slots,
    state: &Solium,
    owner: &crate::effect::plan::Owner,
    resolved: crate::effect::rules::Resolved,
) {
    use crate::effect::rules::Slot;
    for slot in [Slot::Behind, Slot::Front, Slot::Replace] {
        if let Some(key) = resolved.get(slot)
            && state.chains.get(key).is_some()
        {
            slots.want(owner.clone(), slot, key);
        }
    }
}

/// What a slot's owner is captured from, owned so the state is free while
/// its job is built: [`PartSource`]'s parts, and the titlebar, whose own
/// pixels wait for P15 and which has a box but no capture.
/// `state::tests::real_client::only_a_chain_reading_its_part_asks_for_a_capture`.
#[derive(Clone, Debug)]
pub(crate) enum Found {
    Client(Window),
    Popups(Window),
    Layer(crate::pane::PaneId, usize),
    Pane(Window),
    Surface(crate::scripted::SurfaceId, smithay::output::Output),
    LayerShell(LayerSurface),
    Titlebar(crate::pane::PaneId),
}

impl Found {
    /// What it is captured from; `None` for the titlebar.
    pub(crate) fn source(&self) -> Option<PartSource<'_>> {
        Some(match self {
            Self::Client(window) => PartSource::Client(window),
            Self::Popups(window) => PartSource::Popups(window),
            Self::Layer(pane, index) => PartSource::Layer(*pane, *index),
            Self::Pane(window) => PartSource::Pane(window),
            Self::Surface(id, output) => PartSource::Surface(*id, output),
            Self::LayerShell(surface) => PartSource::LayerShell(surface),
            Self::Titlebar(_) => return None,
        })
    }
}

/// What a slot's owner is captured from, and its monitor's scale, which a
/// self input is captured at (Ruling 16): a pane's, a scripted surface's
/// output's, a layer surface's output's. `None` for an owner that has gone,
/// and for a pane's client part while it shows its scene (Ruling 15).
/// `state::tests::real_client::every_wanted_slot_has_its_part_box_padded_by_its_reach`.
fn part_of(state: &Solium, owner: &crate::effect::plan::Owner) -> Option<(Found, f64)> {
    use crate::effect::plan::Owner;
    use smithay::reexports::wayland_server::Resource as _;
    let scale_of = |output: &smithay::output::Output| output.current_scale().fractional_scale();
    match owner {
        Owner::Pane(pane, part) => {
            let held = state.panes.get(*pane)?;
            let scale = state.scale_of(state.pane_outer(held));
            let client = || held.client().cloned();
            let found = match part {
                PaneSlot::Client => Found::Client(client()?),
                PaneSlot::Popups => Found::Popups(client()?),
                PaneSlot::Pane => Found::Pane(client()?),
                PaneSlot::Layer(index) => Found::Layer(*pane, *index),
                PaneSlot::Titlebar => Found::Titlebar(*pane),
            };
            Some((found, scale))
        }
        Owner::Surface(id, name) => {
            let output = state
                .space
                .outputs()
                .find(|output| output.name() == *name)?;
            Some((Found::Surface(*id, output.clone()), scale_of(output)))
        }
        Owner::LayerShell(object) => state.space.outputs().find_map(|output| {
            let layer = layer_map_for_output(output)
                .layers()
                .find(|layer| layer.wl_surface().id() == *object)
                .cloned()?;
            Some((Found::LayerShell(layer), scale_of(output)))
        }),
    }
}

/// How far a part's self capture is padded: its chain's reach on its
/// monitor, rounded up to a whole physical pixel (Ruling 16).
/// `state::tests::real_client::every_wanted_slot_has_its_part_box_padded_by_its_reach`.
fn padding(reach: f64, scale: f64) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a reach in pixels, far below 2^31"
    )]
    let pad = (reach * scale).ceil().max(0.0) as i32;
    pad
}

/// A part's mask radii in physical pixels at `scale`, top-left, top-right,
/// bottom-left, bottom-right ([`mask_radii`]): the client's own rounding,
/// the pane's largest at every corner, none for the rest.
/// `tests::a_parts_radii_are_its_masks_at_its_scale`.
pub(crate) fn part_radii(state: &Solium, source: &PartSource<'_>, scale: f64) -> [f32; 4] {
    let rounding = |window: &Window| {
        state
            .panes
            .id_of(window)
            .and_then(|pane| declared_rounding(state, pane))
            .map(|effect| effect.radii())
    };
    match source {
        PartSource::Client(window) => mask_radii(rounding(window), false, scale),
        PartSource::Pane(window) => mask_radii(rounding(window), true, scale),
        PartSource::Popups(_)
        | PartSource::Layer(..)
        | PartSource::Surface(..)
        | PartSource::LayerShell(_) => [0.0; 4],
    }
}

/// A rounded part's mask radii at `scale`, as a run reads them (top-left,
/// top-right, bottom-left, bottom-right): its own corners, or for a whole
/// pane its largest at every corner, as `effect::mask::client_mask` and
/// `pane_mask` cut; square with no rounding.
/// `tests::a_parts_radii_are_its_masks_at_its_scale`.
pub(crate) fn mask_radii(rounding: Option<Corners>, whole_pane: bool, scale: f64) -> [f32; 4] {
    let corners = match rounding {
        None => Corners::all(0.0),
        Some(radii) if whole_pane => Corners::all(radii.largest()),
        Some(radii) => radii,
    };
    radii_of(corners_times(corners, scale))
}

/// Corners as a run reads them, top-left, top-right, bottom-left,
/// bottom-right. `tests::a_parts_radii_are_its_masks_at_its_scale`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a radius in pixels, given to GL as a float"
)]
fn radii_of(radii: Corners) -> [f32; 4] {
    [
        radii.top_left as f32,
        radii.top_right as f32,
        radii.bottom_left as f32,
        radii.bottom_right as f32,
    ]
}

/// The titlebar's box ([`region_box`] over the insets' regions): it has no
/// capture until P15, but a chain reading nothing of the frame still runs
/// over its box.
fn titlebar_box(
    state: &Solium,
    pane: crate::pane::PaneId,
    scale: f64,
    pad: i32,
) -> Option<crate::effect::plan::PartBox> {
    let held = state.panes.get(pane)?;
    let regions = crate::effect::mask::FromInsets {
        insets: held.decoration()?.insets(),
        outer: state.pane_outer(held).size,
        radii: declared_rounding(state, pane).map_or(Corners::all(0.0), |effect| effect.radii()),
    };
    region_box(&regions, "titlebar", scale, pad)
}

/// A named region's box: its band at `scale`, padded by `pad`, its corners
/// as the region gives them. `None` for a region the source does not have.
/// `tests::the_titlebars_box_is_its_band_padded_with_its_outer_corners`.
pub(crate) fn region_box(
    regions: &dyn crate::effect::mask::RegionSource,
    name: &str,
    scale: f64,
    pad: i32,
) -> Option<crate::effect::plan::PartBox> {
    let (band, radii) = regions.region(name)?;
    Some(crate::effect::plan::PartBox::around(
        crate::offscreen::client_pixels(band.size, scale),
        pad,
        radii_of(corners_times(radii, scale)),
    ))
}

/// Record every wanted slot's padded box, whatever its tier: the part's own
/// pixels on its monitor, padded by its chain's reach, the part inside it,
/// its radii. GPU-free; what the runs read whether or not a capture was
/// drawn this pass. Nothing with no slot wanted.
/// `state::tests::real_client::every_wanted_slot_has_its_part_box_padded_by_its_reach`.
pub(crate) fn record_boxes(state: &Solium, slots: &mut Slots) {
    if slots.is_empty() {
        return;
    }
    let wanted: Vec<(
        crate::effect::plan::Owner,
        Slot,
        crate::effect::rules::RuleKey,
    )> = slots
        .wants()
        .map(|(owner, slot, key)| (owner.clone(), slot, key))
        .collect();
    for (owner, slot, key) in wanted {
        let Some(chain) = state.chains.get(key) else {
            continue;
        };
        let Some((found, scale)) = part_of(state, &owner) else {
            continue;
        };
        let pad = padding(chain.reach, scale);
        let part = match (&found, found.source()) {
            (_, Some(source)) => crate::offscreen::part_box(state, &source, scale, pad),
            (Found::Titlebar(pane), None) => titlebar_box(state, *pane, scale, pad),
            (_, None) => None,
        };
        if let Some(part) = part {
            slots.set_box(owner, slot, part);
        }
    }
}

/// One slot's self input to capture this pass.
/// `state::tests::real_client::only_a_chain_reading_its_part_asks_for_a_capture`.
#[derive(Debug)]
pub(crate) struct SelfInput {
    pub(crate) owner: crate::effect::plan::Owner,
    pub(crate) slot: Slot,
    found: Found,
    scale: f64,
    pad: i32,
}

/// The wanted slots whose chain reads its part's own pixels (T1,
/// `Tier::Own`), each with what its part is captured from, at its monitor's
/// scale, padded by its chain's reach: what `prepare` captures. A chain
/// reading nothing of the frame (T0) asks for no capture, and nor does the
/// titlebar, whose self rules are refused until P15.
/// `state::tests::real_client::only_a_chain_reading_its_part_asks_for_a_capture`.
pub(crate) fn self_inputs(state: &Solium, slots: &Slots) -> Vec<SelfInput> {
    slots
        .wants()
        .filter_map(|(owner, slot, key)| {
            let chain = state.chains.get(key)?;
            if chain.tier != crate::effect::rules::Tier::Own {
                return None;
            }
            let (found, scale) = part_of(state, owner)?;
            found.source()?;
            Some(SelfInput {
                owner: owner.clone(),
                slot,
                found,
                scale,
                pad: padding(chain.reach, scale),
            })
        })
        .collect()
}

/// The fields of `Solium` a pass's chain runs touch, borrowed apart so the
/// pool can be lent to [`with_carrier`] beside them.
/// `tests::a_self_slot_with_no_input_runs_nothing_and_opens_no_region`.
pub(crate) struct RunCx<'a> {
    pub(crate) store: &'a mut crate::effect::store::Store,
    pub(crate) effects: &'a crate::effect::host::Host,
    pub(crate) chains: &'a mut crate::effect::plan::Chains,
    /// What a result is drawn through (`pass::Programs::masked`, compiled at
    /// the top of `prepare` while a slot is wanted); with none, no slot is
    /// ready and the walk draws as if every chain had failed.
    pub(crate) masked: Option<GlesTexProgram>,
    /// Seconds, from the one clock (#164).
    pub(crate) now: f32,
    /// The store's pass, which a slot run in it is marked with.
    pub(crate) pass: u64,
}

impl std::fmt::Debug for RunCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunCx")
            .field("now", &self.now)
            .field("pass", &self.pass)
            .finish_non_exhaustive()
    }
}

/// What runs a chain: the GPU in `prepare`, `tests::NoGpu` in the tests.
pub(crate) trait Runner {
    /// Before a phase's first run: `GlRunner` opens the phase's one
    /// `Region::Effect` (Ruling 10).
    fn begin(&mut self);
    #[expect(
        clippy::too_many_arguments,
        reason = "run::run's own, less the renderer and carrier the runner holds"
    )]
    fn run<'p>(
        &mut self,
        pool: &mut crate::pool::Pool,
        programs: &dyn Fn(u64) -> crate::effect::run::Lookup<'p>,
        formats: Option<crate::pool::Formats>,
        plan: &solium_effects::stage::Plan,
        held: &mut crate::effect::run::Held,
        inputs: &crate::effect::run::Inputs<'_>,
        keys: &crate::effect::run::Keys,
    ) -> crate::effect::run::Outcome;
    /// After a phase's last run: closes the region.
    fn end(&mut self);
    /// What a generated (T0) chain's first input reads, which names no frame
    /// texture: `GlRunner`'s is the pool's 1x1 transparent texture
    /// (`pool::Pool::blank`), `tests::NoGpu`'s none.
    /// `tests::twenty_runs_in_a_phase_open_one_region`, `dev/effects-check.sh t0`.
    fn blank(&mut self, pool: &mut crate::pool::Pool) -> Option<GlesTexture>;
}

/// The GPU's runner: `run::run` on the carrier [`with_carrier`] bound, the
/// phase timed as one region.
pub(crate) struct GlRunner<'r, 'c> {
    renderer: &'r mut GlesRenderer,
    carrier: &'r mut smithay::backend::renderer::gles::GlesTarget<'c>,
    timer: Option<&'r mut crate::gputime::Timer>,
    open: Option<crate::gputime::Stamp>,
}

impl std::fmt::Debug for GlRunner<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlRunner")
            .field("timed", &self.timer.is_some())
            .field("open", &self.open.is_some())
            .finish_non_exhaustive()
    }
}

impl<'r, 'c> GlRunner<'r, 'c> {
    pub(crate) fn new(
        renderer: &'r mut GlesRenderer,
        carrier: &'r mut smithay::backend::renderer::gles::GlesTarget<'c>,
        timer: Option<&'r mut crate::gputime::Timer>,
    ) -> Self {
        Self {
            renderer,
            carrier,
            timer,
            open: None,
        }
    }
}

impl Runner for GlRunner<'_, '_> {
    fn begin(&mut self) {
        if self.open.is_none() {
            self.open = self
                .timer
                .as_deref_mut()
                .map(|timer| timer.open(self.renderer, crate::gputime::Region::Effect));
        }
    }

    fn run<'p>(
        &mut self,
        pool: &mut crate::pool::Pool,
        programs: &dyn Fn(u64) -> crate::effect::run::Lookup<'p>,
        formats: Option<crate::pool::Formats>,
        plan: &solium_effects::stage::Plan,
        held: &mut crate::effect::run::Held,
        inputs: &crate::effect::run::Inputs<'_>,
        keys: &crate::effect::run::Keys,
    ) -> crate::effect::run::Outcome {
        crate::effect::run::run(
            self.renderer,
            self.carrier,
            pool,
            programs,
            formats,
            plan,
            held,
            inputs,
            keys,
        )
    }

    fn end(&mut self) {
        if let (Some(timer), Some(stamp)) = (self.timer.as_deref_mut(), self.open.take()) {
            timer.close(self.renderer, stamp);
        }
    }

    fn blank(&mut self, pool: &mut crate::pool::Pool) -> Option<GlesTexture> {
        pool.blank(self.renderer)
    }
}

/// Bind the pool's carrier once and hand it, the renderer and the pool to
/// `f`, as `offscreen::draw` binds it for its jobs, then put framebuffer 0
/// back as it does (`warp::release_framebuffer`). `None` with no carrier or
/// one that would not bind. Nothing in `f` may touch a QML scene.
pub(crate) fn with_carrier<R>(
    pool: &mut crate::pool::Pool,
    renderer: &mut GlesRenderer,
    f: impl FnOnce(
        &mut GlesRenderer,
        &mut smithay::backend::renderer::gles::GlesTarget<'_>,
        &mut crate::pool::Pool,
    ) -> R,
) -> Option<R> {
    use smithay::backend::renderer::Bind as _;
    let Some(mut carrier) = pool.carrier(renderer) else {
        tracing::warn!("no carrier to run this pass's effects on");
        return None;
    };
    let done = match renderer.bind(&mut carrier) {
        Err(err) => {
            tracing::warn!(?err, "could not bind the effects' carrier");
            None
        }
        Ok(mut bound) => {
            let _frame = crate::qml::frame_in_flight();
            Some(f(renderer, &mut bound, pool))
        }
    };
    crate::warp::release_framebuffer(renderer);
    done
}

/// The key a `depends = "shape"` state is rebuilt on: the part's box in the
/// padded box and its radii, by their bits.
/// `tests::the_shape_key_moves_with_the_box_and_the_radii`.
pub(crate) fn shape_hash(content: [f32; 4], radii: [f32; 4]) -> u64 {
    let bytes: Vec<u8> = content
        .iter()
        .chain(radii.iter())
        .flat_map(|each| each.to_bits().to_le_bytes())
        .collect();
    solium_effects::glsl::content_hash(&[&bytes])
}

/// Which of `prepare`'s two run phases a slot's chain runs in (Ruling 17):
/// a whole pane's self chain in the later one, since its capture walks what
/// the inner phase made ([`inner_pieces`]), and every other in the first.
/// `tests::only_a_whole_panes_self_chain_runs_in_the_later_phase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Nest {
    Inner,
    Whole,
}

impl Nest {
    /// `tests::only_a_whole_panes_self_chain_runs_in_the_later_phase`.
    pub(crate) fn of(owner: &crate::effect::plan::Owner, tier: crate::effect::rules::Tier) -> Self {
        match (owner, tier) {
            (
                crate::effect::plan::Owner::Pane(_, PaneSlot::Pane),
                crate::effect::rules::Tier::Own,
            ) => Self::Whole,
            _ => Self::Inner,
        }
    }
}

/// Whether a slot's first input counts as redrawn this pass: only a self
/// (T1) input that was captured anew. A generated (T0) chain reads nothing
/// of the frame, so nothing committing under it re-runs it; only its params
/// or its padded size do (`SlotState::needs_run`).
/// `tests::a_t0_slot_never_counts_as_redrawn`,
/// `effect::store::tests::a_generated_effect_runs_once_and_not_again_until_its_size_or_params_change`.
pub(crate) fn input_redrawn(tier: crate::effect::rules::Tier, captured: bool) -> bool {
    tier == crate::effect::rules::Tier::Own && captured
}

/// Run every wanted slot of `nest` whose tier runs here, each only when
/// `needs_run` says so, else its last result placed again with the same id
/// and commit (Ruling 16). A T1 slot reads its self input from `drawn`
/// (redrawn this pass, or kept); with neither there is nothing to run and
/// the part is drawn plain. A T0 slot reads the runner's blank and is never
/// redrawn. One `begin` before the phase's first run and one `end` after its
/// last (Ruling 10).
/// `tests::a_self_slot_with_no_input_runs_nothing_and_opens_no_region`,
/// `tests::twenty_runs_in_a_phase_open_one_region`,
/// `effect::store::tests::a_self_chain_runs_once_until_its_part_commits`.
pub(crate) fn run_slots(
    cx: &mut RunCx<'_>,
    pool: &mut crate::pool::Pool,
    runner: &mut dyn Runner,
    slots: &mut Slots,
    drawn: &crate::effect::store::Drawn,
    nest: Nest,
) {
    use crate::effect::rules::Tier;
    use crate::effect::run::{BoxMap, Inputs, Keys, Outcome};
    let wanted: Vec<(
        crate::effect::plan::Owner,
        Slot,
        crate::effect::rules::RuleKey,
    )> = slots
        .wants()
        .map(|(owner, slot, key)| (owner.clone(), slot, key))
        .collect();
    let mut began = false;
    for (owner, slot, key) in wanted {
        let (Some(chain), Some(part)) = (cx.chains.get(key), slots.boxed(&owner, slot)) else {
            continue;
        };
        if Nest::of(&owner, chain.tier) != nest {
            continue;
        }
        let (first, redrawn) = match chain.tier {
            Tier::Own => match drawn.get(&owner, slot) {
                Some((texture, captured)) => {
                    (Some(texture.clone()), input_redrawn(chain.tier, captured))
                }
                None => continue,
            },
            // A generated chain reads nothing of the frame: no capture, never
            // redrawn, its first input the pool's blank, so it runs on its
            // first pass and again only when its params or its padded size
            // change (`tests::twenty_runs_in_a_phase_open_one_region`,
            // `effect::store::tests::a_generated_effect_runs_once_and_not_again_until_its_size_or_params_change`).
            Tier::Generated => (runner.blank(pool), input_redrawn(chain.tier, false)),
            // T2 and T3 were refused at load (Ruling 14).
            Tier::Xray | Tier::Live => continue,
        };
        let size = part.size();
        let params = chain.params_hash;
        let keys = Keys {
            params,
            own: drawn.commit(&owner, slot),
            shape: shape_hash(part.content, part.radii),
        };
        let state = cx.store.slot_mut(&owner, slot, key);
        state.seen = cx.pass;
        if !state.needs_run(redrawn, params, size) {
            if let Some(texture) = state.held.output().cloned()
                && let Some(ready) = ready_from(state, cx.masked.as_ref(), texture, false, part)
            {
                slots.set_ready(owner, slot, ready);
            }
            continue;
        }
        let textures: Vec<(&str, GlesTexture, BoxMap)> = first
            .into_iter()
            .map(|texture| (chain.plan.first_input.as_str(), texture, BoxMap::WHOLE))
            .collect();
        let inputs = Inputs {
            padded: size,
            content: part.content,
            textures: &textures,
            radii: part.radii,
            time: cx.now,
            transition: crate::effect::run::Transition::default(),
            // Ruling 6's edge rule: a self chain in `replace` reads the
            // part's own edge beyond it, in `behind` and `front` transparent
            // (`effect::run::tests::the_first_input_is_clamped_to_its_edge_texels`).
            clamp_first: slot == Slot::Replace && chain.tier == Tier::Own,
        };
        if !began {
            runner.begin();
            began = true;
        }
        let effects = cx.effects;
        let outcome = runner.run(
            pool,
            &|key| effects.lookup(key),
            effects.formats(),
            &chain.plan,
            &mut state.held,
            &inputs,
            &keys,
        );
        match outcome {
            Outcome::Done(texture, sync) => {
                let _ = crate::offscreen::settle(
                    Ok::<_, std::convert::Infallible>(sync),
                    crate::dev::fence_wait(),
                    "an effect",
                );
                crate::pacing::effect_ran();
                state.ran(params, size);
                if let Some(ready) = ready_from(state, cx.masked.as_ref(), texture, true, part) {
                    slots.set_ready(owner, slot, ready);
                }
            }
            // Its program is not compiled yet, or its format waits for the
            // rebind: the part is drawn plain this pass, and nothing is
            // latched or said (Ruling 10,
            // `effect::run::tests::a_program_not_compiled_yet_is_pending_not_failed`).
            Outcome::Pending => {}
            // Latched in `Held`, said once per rule, and no `Ready`: the walk
            // draws the part (`replace`) or nothing (`behind`, `front`), [16]
            // §5 (`tests::a_wanted_slot_with_nothing_ready_draws_what_no_slot_draws`,
            // `effect::plan::tests::a_failed_chain_is_said_once_per_rule`).
            Outcome::Failed => {
                let _ = cx.chains.refuse_once(
                    key,
                    "an effect's chain failed; its part is drawn without it",
                );
            }
        }
    }
    if began {
        runner.end();
    }
}

/// A slot's result as the walk places it: its texture in an
/// `EffectElement` with the slot's own id, its commit moved only when the
/// chain re-ran or its box changed, drawn through the masked program; `None`
/// with no program, and then the walk draws as if the chain had failed.
/// `effect::store::tests::an_unchanged_chain_keeps_its_outputs_id_and_commit`.
fn ready_from(
    state: &mut crate::effect::store::SlotState,
    masked: Option<&GlesTexProgram>,
    texture: GlesTexture,
    rerun: bool,
    part: crate::effect::plan::PartBox,
) -> Option<crate::effect::plan::Ready> {
    let program = masked?.clone();
    let commit = state.commit_for(
        crate::effect::element::Placement::of(part.padded, None, 1.0),
        rerun,
    );
    Some(crate::effect::plan::Ready {
        element: crate::effect::element::EffectElement::new(
            state.id.clone(),
            commit,
            texture,
            program,
        ),
        padded: part.padded,
        reach: part.reach,
    })
}

/// The rounding a pane's style declares, if any: the one inline effect today.
/// `tests::a_style_with_no_radius_wraps_nothing`, `tests::a_style_with_a_radius_is_drawn_inline`,
/// `tests::a_none_effect_does_not_hide_the_rounding_behind_it`, `tests::of_two_roundings_the_first_wins`.
pub(crate) fn rounding(
    effects: &[solium_effects::fragment::Effect],
) -> Option<solium_effects::fragment::Effect> {
    effects.iter().copied().find(|effect| {
        effect.inputs() == solium_effects::fragment::Inputs::Inline && !effect.is_none_effect()
    })
}

/// The rounding a pane's style declares, read without borrowing the state.
/// A fullscreen pane has no frame, so none: `decoration::tests::a_fullscreen_window_is_drawn_square`.
fn declared_rounding(
    state: &Solium,
    pane: crate::pane::PaneId,
) -> Option<solium_effects::fragment::Effect> {
    rounding(&declared_effects(state, pane))
}

/// What a pane's client is clipped with this frame: its rounding and the
/// programs `prepare` compiled, or `None` (the path every unstyled window
/// takes, untouched). **Never compiles**: `elements` may run with an output
/// bound, where a compile's `make_current` is the frozen-compositor failure
/// `Prepared`'s doc describes; `prepare` compiles them, between frames.
fn clipped(
    state: &Solium,
    pane: crate::pane::PaneId,
) -> Option<(solium_effects::fragment::Effect, crate::pass::ClipPrograms)> {
    let effect = declared_rounding(state, pane)?;
    Some((effect, state.programs.clip_compiled()?.clone()))
}

/// A client's clip on the flat path: the client's rectangle as drawn,
/// `drawn`, taken back through the rescale by `factor` about `origin` into
/// the surfaces' own pixels, where `clip::input_to_geo` measures. At 1:1 it
/// is `drawn` itself. `tests::a_zoomed_clients_clip_is_the_whole_client`.
pub(crate) fn drawn_clip(
    drawn: Rectangle<i32, Physical>,
    origin: Point<i32, Physical>,
    factor: Scale<f64>,
    radii: solium_effects::fragment::Corners,
) -> crate::clip::Clip {
    let back = |at: i32, about: i32, by: f64| f64::from(about) + f64::from(at - about) / by;
    crate::clip::Clip {
        rect: Rectangle::new(
            (
                back(drawn.loc.x, origin.x, factor.x),
                back(drawn.loc.y, origin.y, factor.y),
            )
                .into(),
            (
                f64::from(drawn.size.w) / factor.x,
                f64::from(drawn.size.h) / factor.y,
            )
                .into(),
        ),
        radii,
        origin,
        factor,
    }
}

/// A client's clip inside a warp's capture: the frame leaves it `hole` from
/// `origin` (`room`), and its surface tree is drawn at `tree_origin`, so its
/// `geometry` (relative to the tree) lands at `tree_origin + geometry.loc`
/// (`client`) -- cut to `room`, which is what the capture holds of it.
///
/// **Two origins, not one.** `tree_origin` differs from `origin` exactly when
/// `window_surface_origin` has shifted the tree off the frame's own origin to
/// re-place a CSD client's content (#232): `room` has to stay anchored at the
/// frame's real origin regardless, or it shrinks by that same shift and crops
/// the content short on top of repositioning it, which is not what either fix
/// is for. `tests::in_a_capture_a_client_is_clipped_to_what_the_capture_holds_of_it`,
/// `tests::a_shifted_tree_does_not_shrink_the_room_its_client_is_cut_to`.
pub(crate) fn capture_clip(
    origin: Point<i32, Physical>,
    hole: Size<i32, Physical>,
    tree_origin: Point<i32, Physical>,
    geometry: Rectangle<i32, Physical>,
    radii: solium_effects::fragment::Corners,
) -> crate::clip::Clip {
    let room = Rectangle::new(origin, hole);
    let client = Rectangle::new(tree_origin + geometry.loc, geometry.size);
    crate::clip::Clip {
        rect: client.intersection(room).unwrap_or(room).to_f64(),
        radii,
        origin,
        factor: Scale::from(1.0),
    }
}

/// Which capture a pane wants this pass: its warp's, or none, which is every
/// unwarped window, rounded or not. `tests::a_pane_neither_warped_nor_styled_wants_no_capture`.
pub(crate) fn wanted_capture(route: Route) -> Option<crate::keyed::Kind> {
    match route {
        Route::Warp => Some(crate::keyed::Kind::Pane),
        Route::Flat | Route::Hidden => None,
    }
}

/// Everything to draw this frame, topmost first.
///
/// Topmost first is what the damage tracker expects; getting it backwards
/// composites the stack upside down, which looks like a stacking bug rather
/// than an ordering one.
/// One piece of a pane, in the order it goes into the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    /// The style's layers at one depth, topmost first.
    Layers(Depth),
    /// The client's own surface and the popups above it — or, for a pane whose
    /// application has not arrived, the scene standing in for one.
    Client,
    /// An effect's slot around a part of the pane (Ruling 15). Only
    /// [`pane_walk`] yields one.
    Slot(PaneSlot, Slot),
}

/// A pane's pieces, topmost first: `above`, `frame`, the client, `behind`.
///
/// **The order is stated here and nowhere else.** It is the whole point of the
/// feature — a client's surface sitting *between* two layers the same style
/// produced is the one thing a single QML file cannot do — and it is also the
/// easiest thing in it to get wrong by half a list, in a way that reads as a
/// stacking bug rather than an ordering one. The pane's own position among the
/// other panes is untouched: this is only what happens inside one of them.
///
/// Three walks read it: `elements` for a live client, `elements` again for a
/// pane still showing the compositor's own scene, and [`flat_window_elements`]
/// for the offscreen pass a deformed window is drawn through. A tilted window
/// has to carry its layers in the order it would have had flat, which is the
/// second reason this is one array rather than three sequences of calls.
pub(crate) const PANE_ORDER: [Piece; 4] = [
    Piece::Layers(Depth::Above),
    Piece::Layers(Depth::Frame),
    Piece::Client,
    Piece::Layers(Depth::Behind),
];

/// Walk one pane's pieces in the order they go into the frame.
///
/// Generic over what a piece produces, and taking the list to push into rather
/// than returning one, for two separate reasons. The first is cost: the
/// identity case has to stay exactly what a decoration costs today, and a `Vec`
/// returned per depth per pane per frame is an allocation that did not exist
/// before. The second is that the compositor's own walk can then be *driven* by
/// a test with no renderer and no GPU — the same [`PANE_ORDER`], the same
/// `Decoration`, and a closure that records a layer's name instead of building
/// an element out of it.
pub(crate) fn pane_pieces<T>(into: &mut Vec<T>, mut piece: impl FnMut(&mut Vec<T>, Piece)) {
    for each in PANE_ORDER {
        piece(into, each);
    }
}

/// A pane's pieces with its slots, topmost first (Ruling 15): the pane's
/// `front`, then `PANE_ORDER` with the client's slots around the client, then
/// the pane's `behind`; a pane `replace` is the whole of it; a client
/// `replace` takes the client's place. Layer and titlebar slots are inside
/// `Layers`, around their layer ([`around_layer`]), and the popups' around
/// the popups, which go in ahead of the walk.
/// `tests::with_no_slots_the_pane_walk_is_pane_order`,
/// `tests::client_slots_bracket_the_client_and_replace_takes_its_place`,
/// `tests::pane_slots_are_around_the_sandwich_and_below_the_popups`.
pub(crate) fn pane_walk<T>(
    into: &mut Vec<T>,
    has: &dyn Fn(PaneSlot, Slot) -> bool,
    mut piece: impl FnMut(&mut Vec<T>, Piece),
) {
    if has(PaneSlot::Pane, Slot::Replace) {
        piece(into, Piece::Slot(PaneSlot::Pane, Slot::Replace));
        return;
    }
    if has(PaneSlot::Pane, Slot::Front) {
        piece(into, Piece::Slot(PaneSlot::Pane, Slot::Front));
    }
    for each in PANE_ORDER {
        match each {
            Piece::Client => {
                if has(PaneSlot::Client, Slot::Replace) {
                    piece(into, Piece::Slot(PaneSlot::Client, Slot::Replace));
                    continue;
                }
                if has(PaneSlot::Client, Slot::Front) {
                    piece(into, Piece::Slot(PaneSlot::Client, Slot::Front));
                }
                piece(into, Piece::Client);
                if has(PaneSlot::Client, Slot::Behind) {
                    piece(into, Piece::Slot(PaneSlot::Client, Slot::Behind));
                }
            }
            other => piece(into, other),
        }
    }
    if has(PaneSlot::Pane, Slot::Behind) {
        piece(into, Piece::Slot(PaneSlot::Pane, Slot::Behind));
    }
}

/// The layer the titlebar's slots bracket: the one named `bar`, else the
/// first `frame` layer, else none.
/// `tests::titlebar_slots_bracket_the_bar_layer`.
pub(crate) fn titlebar_layer(layers: &[(usize, Depth, &str)]) -> Option<usize> {
    layers
        .iter()
        .find(|(_, _, name)| *name == "bar")
        .or_else(|| layers.iter().find(|(_, depth, _)| *depth == Depth::Frame))
        .map(|(index, _, _)| *index)
}

/// Where a slot around one of a style's layers goes: just before the layer
/// (over it), in its place, or just after it (under it).
/// `tests::around_a_layer_its_own_slots_are_outermost_and_the_titlebars_inside`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Around {
    Before,
    InPlace,
    After,
}

/// The slots around the style's layer at `index`, topmost first, and where
/// each goes: the layer's own outermost, and the titlebar's inside them when
/// `titlebar` names this layer, a titlebar being a band of its layer. A
/// layer's `replace` goes in its place; the titlebar's goes over the layer.
/// `tests::around_a_layer_its_own_slots_are_outermost_and_the_titlebars_inside`.
pub(crate) fn around_layer(
    index: usize,
    titlebar: Option<usize>,
) -> impl Iterator<Item = (PaneSlot, Slot, Around)> {
    let bar = titlebar == Some(index);
    let layer = PaneSlot::Layer(index);
    [
        Some((layer, Slot::Front, Around::Before)),
        bar.then_some((PaneSlot::Titlebar, Slot::Front, Around::Before)),
        bar.then_some((PaneSlot::Titlebar, Slot::Replace, Around::Before)),
        Some((layer, Slot::Replace, Around::InPlace)),
        bar.then_some((PaneSlot::Titlebar, Slot::Behind, Around::After)),
        Some((layer, Slot::Behind, Around::After)),
    ]
    .into_iter()
    .flatten()
}

/// `Decoration::layer_elements`' hook over the slots [`layer_slots`] placed:
/// before the layer at `index`, those that go before it; after it, its
/// `replace` in place of what the layer pushed, then those that go after.
/// Each is pushed once. `tests::a_layers_replace_takes_its_place_and_its_other_slots_go_around_it`.
pub(crate) fn place_around<T>(
    placed: &mut [(usize, Around, Option<T>)],
    mark: &mut usize,
    into: &mut Vec<T>,
    index: usize,
    before: bool,
) {
    let mut push = |into: &mut Vec<T>, around: Around| {
        into.extend(
            placed
                .iter_mut()
                .filter(|(at, place, _)| *at == index && *place == around)
                .filter_map(|(_, _, element)| element.take()),
        );
    };
    if before {
        push(into, Around::Before);
        *mark = into.len();
        return;
    }
    let len = into.len();
    push(into, Around::InPlace);
    if into.len() > len {
        into.drain(*mark..len);
    }
    push(into, Around::After);
}

/// Whether a pane's slot has a result to draw: readiness and not wanting,
/// so a wanted slot whose chain failed, or has not run, draws the part
/// (`replace`) or nothing (`behind`, `front`). The one predicate the pane
/// walk asks. `tests::a_wanted_slot_with_nothing_ready_draws_what_no_slot_draws`.
pub(crate) fn slot_ready(
    slots: &Slots,
    pane: crate::pane::PaneId,
) -> impl Fn(PaneSlot, Slot) -> bool + '_ {
    move |part, slot| slots.is_ready(&crate::effect::plan::Owner::Pane(pane, part), slot)
}

/// A warped pane's capture: every slot ready this pass, in the flat path's
/// order. `tests::a_warped_panes_capture_walks_its_client_slot`.
pub(crate) fn capture_pieces(slots: &Slots, pane: crate::pane::PaneId) -> Vec<Piece> {
    let mut pieces = Vec::new();
    pane_walk(&mut pieces, &slot_ready(slots, pane), |into, piece| {
        into.push(piece);
    });
    pieces
}

/// A whole pane's self capture: its inner parts' results, never its own
/// `pane` slots, which would read the last pass's result of the effect being
/// run. `tests::a_whole_panes_capture_walks_its_inner_slots_and_not_its_own`.
pub(crate) fn inner_pieces(slots: &Slots, pane: crate::pane::PaneId) -> Vec<Piece> {
    let ready = slot_ready(slots, pane);
    let mut pieces = Vec::new();
    pane_walk(
        &mut pieces,
        &|part, slot| part != PaneSlot::Pane && ready(part, slot),
        |into, piece| into.push(piece),
    );
    pieces
}

/// Whether a run phase has a chain to run: a wanted slot whose bound chain
/// runs in `nest`. One that has none binds no carrier (Ruling 17).
/// `tests::a_run_phase_with_no_chain_of_its_own_binds_nothing`.
pub(crate) fn runs_in(slots: &Slots, chains: &crate::effect::plan::Chains, nest: Nest) -> bool {
    slots.wants().any(|(owner, _, key)| {
        chains
            .get(key)
            .is_some_and(|chain| Nest::of(owner, chain.tier) == nest)
    })
}

/// Whether a pane drawn over `drawn` reaches `screen` once grown by `reach`
/// on every side, the furthest its slots' results reach past it: the bleed
/// cull. `tests::the_bleed_cull_counts_an_effects_reach`.
pub(crate) fn reaches(
    drawn: Rectangle<f64, Logical>,
    reach: i32,
    screen: Rectangle<i32, Logical>,
) -> bool {
    let reach = f64::from(reach);
    Rectangle::<f64, Logical>::new(
        (drawn.loc.x - reach, drawn.loc.y - reach).into(),
        (drawn.size.w + 2.0 * reach, drawn.size.h + 2.0 * reach).into(),
    )
    .overlaps(screen.to_f64())
}

/// Where a ready slot's result goes on an output: its padded box over
/// `part`, the part's own rectangle there in physical pixels, grown by the
/// result's `reach` scaled as the part is drawn against the size it was
/// padded at; and the part, with its `radii`, as the mask inside that box.
/// `tests::a_slot_is_placed_over_its_part_grown_by_its_reach_as_the_part_is_drawn`.
pub(crate) fn slot_placement(
    part: Rectangle<f64, Physical>,
    padded: Size<i32, Physical>,
    reach: i32,
    radii: Corners,
) -> (
    Rectangle<i32, Physical>,
    (Rectangle<f64, Physical>, Corners),
) {
    let own = |padded: i32| f64::from((padded - 2 * reach).max(1));
    let across = f64::from(reach) * part.size.w / own(padded.w);
    let down = f64::from(reach) * part.size.h / own(padded.h);
    let dst = Rectangle::<f64, Physical>::new(
        (part.loc.x - across, part.loc.y - down).into(),
        (part.size.w + 2.0 * across, part.size.h + 2.0 * down).into(),
    )
    .to_i32_round();
    let mask = Rectangle::new(
        (
            part.loc.x - f64::from(dst.loc.x),
            part.loc.y - f64::from(dst.loc.y),
        )
            .into(),
        part.size,
    );
    (dst, (mask, radii))
}

/// Whether a slot's result is cut by its part's shape (\[16\] §2): unless
/// its rule asks for the self capture's alpha, or its chain reads `shape`
/// and so draws its own edges.
/// `tests::a_result_is_cut_by_its_parts_shape_unless_it_owns_its_edges`.
pub(crate) fn cut_by_shape(mask: MaskKind, reads_shape: bool) -> bool {
    mask == MaskKind::Shape && !reads_shape
}

/// Every corner times `by`.
fn corners_times(radii: Corners, by: f64) -> Corners {
    Corners {
        top_left: radii.top_left * by,
        top_right: radii.top_right * by,
        bottom_left: radii.bottom_left * by,
        bottom_right: radii.bottom_right * by,
    }
}

/// A ready slot's result placed over its part, `part` being the part's
/// mask on this output in logical pixels (`effect::mask`), cut by it as
/// [`cut_by_shape`] says, at `alpha`; nothing for a slot with no result.
fn slot_element(
    state: &Solium,
    slots: &Slots,
    owner: &crate::effect::plan::Owner,
    slot: Slot,
    part: Mask,
    scale: f64,
    alpha: f32,
) -> Option<Element> {
    let ready = slots.at(owner, slot)?;
    let Mask::Rect { rect, radii } = part else {
        return None;
    };
    let key = slots.key(owner, slot)?;
    let style = match owner {
        crate::effect::plan::Owner::Pane(pane, _) => state
            .panes
            .get(*pane)
            .map_or(&[][..], |held| style_rules(held.frame()).0),
        crate::effect::plan::Owner::Surface(..) | crate::effect::plan::Owner::LayerShell(_) => &[],
    };
    let cut = cut_by_shape(
        state
            .rules
            .rule(style, key)
            .map_or(MaskKind::Shape, |rule| rule.mask),
        state
            .chains
            .get(key)
            .is_some_and(|chain| chain.plan.reads.shape),
    );
    let (dst, mask) = slot_placement(
        rect.to_physical(scale),
        ready.padded.size,
        ready.reach,
        corners_times(radii, scale),
    );
    Some(Element::Effect(ready.element.at(
        dst,
        cut.then_some(mask),
        alpha,
    )))
}

/// The ready slots around a pane's layers at `depth`, placed, each with its
/// layer's place and where it goes ([`around_layer`]): what `chrome`'s hook
/// pushes ([`place_around`]), made before the decoration is borrowed to draw
/// them.
fn layer_slots(
    state: &Solium,
    slots: &Slots,
    pane: crate::pane::PaneId,
    depth: Depth,
    drawing: crate::decoration::Drawing,
) -> Vec<(usize, Around, Option<Element>)> {
    let mut placed = Vec::new();
    if slots.is_empty() {
        return placed;
    }
    let Some(decoration) = state.panes.get(pane).and_then(Pane::decoration) else {
        return placed;
    };
    let places: Vec<(usize, Depth, &str)> = decoration.layer_places().collect();
    let titlebar = titlebar_layer(&places);
    let regions = crate::effect::mask::FromInsets {
        insets: decoration.insets(),
        outer: drawing.outer,
        radii: declared_rounding(state, pane).map_or(Corners::all(0.0), |effect| effect.radii()),
    };
    let zoom = (
        drawing.rect.size.w / f64::from(drawing.outer.w.max(1)),
        drawing.rect.size.h / f64::from(drawing.outer.h.max(1)),
    );
    for &(index, at, _) in &places {
        if at != depth {
            continue;
        }
        for (part, slot, around) in around_layer(index, titlebar) {
            let owner = crate::effect::plan::Owner::Pane(pane, part);
            if !slots.is_ready(&owner, slot) {
                continue;
            }
            let mask = match part {
                PaneSlot::Titlebar => regions.region("titlebar").map(|(band, radii)| Mask::Rect {
                    rect: Rectangle::new(
                        (
                            drawing.rect.loc.x + f64::from(band.loc.x) * zoom.0,
                            drawing.rect.loc.y + f64::from(band.loc.y) * zoom.1,
                        )
                            .into(),
                        (
                            f64::from(band.size.w) * zoom.0,
                            f64::from(band.size.h) * zoom.1,
                        )
                            .into(),
                    ),
                    radii: corners_times(radii, zoom.0),
                }),
                _ => decoration
                    .layer_canvas(index, drawing)
                    .map(|canvas| Mask::Rect {
                        rect: canvas,
                        radii: Corners::all(0.0),
                    }),
            };
            if let Some(element) = mask.and_then(|mask| {
                slot_element(
                    state,
                    slots,
                    &owner,
                    slot,
                    mask,
                    drawing.scale,
                    drawing.alpha,
                )
            }) {
                placed.push((index, around, Some(element)));
            }
        }
    }
    placed
}

/// The frame around a pane, at one depth.
///
/// Takes the pane's whole transform — through [`crate::decoration::Drawing`] —
/// rather than a rect and an alpha, and that is deliberate: every piece of a
/// window — the client's surface, its popups, its layers, the scene standing in
/// for it — is drawn from the *pane's* transform, so none of them can be given
/// the wrong one or miss one of its parts. The frame did miss the opacity, and
/// a window closing faded away underneath a titlebar that stayed perfectly
/// solid.
///
/// Drawn whenever there is a decoration at all, not only when it reserved
/// space: a frame that takes nothing and floats over the window — a bar that
/// appears on hover, a border that does not push the client around — is a
/// decoration too. And it covers the whole window rather than a strip of it,
/// which is what lets one put its bar on any side, or draw a border, or both.
fn chrome(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    elements: &mut Vec<Element>,
    slots: Option<&Slots>,
    pane: crate::pane::PaneId,
    depth: Depth,
    drawing: crate::decoration::Drawing,
) {
    let title = state.pane_title(pane);
    let look = crate::decoration::Look {
        title: &title,
        focused: state.looks_focused(pane),
        pointer_inside: state.pointer_over(pane),
        // `text_input::tests::only_the_pane_whose_window_has_the_caret_is_given_it`.
        caret: state.caret_in(pane),
        // `decoration::tests::a_layer_is_told_the_configurations_values_and_told_again_when_they_change`.
        values: state.decorations.values(),
    };
    let mut around = slots
        .map(|slots| layer_slots(state, slots, pane, depth, drawing))
        .unwrap_or_default();
    let mut animating = false;
    if let Some(decoration) = state.panes.get_mut(pane).and_then(Pane::decoration_mut) {
        // Already `Element`s: a layer is a memory buffer on the software path
        // and a texture on the GPU one, and which of the two it is is the
        // decoration's own business rather than this function's.
        //
        // And already an answer about whether they are still moving, out of the
        // same call: asking afterwards is asking the flag the draw just spent.
        // See [`Drawn`].
        let mut mark = 0;
        let mut hook = |into: &mut Vec<Element>, index: usize, before: bool| {
            place_around(&mut around, &mut mark, into, index, before);
        };
        animating = decoration.layer_elements(renderer, depth, &look, drawing, elements, &mut hook);
    }
    // Ask for another frame while the decoration is still moving. The client
    // has not damaged anything, so without this the next frame never comes and
    // the animation stops where it stood.
    if animating {
        state.redraw = true;
    }
}

/// The rectangle the layer at `index` of a pane's style is rasterised into,
/// at rest: the pane grown by that layer's bleed (`decoration::canvas`), its
/// corner as far above and left of the pane's as the bleed reaches. What a
/// layer's self capture holds, its corner at the pad
/// (`tests::a_layers_capture_puts_its_canvas_corner_at_the_pad`).
pub(crate) fn layer_canvas(
    state: &Solium,
    pane: crate::pane::PaneId,
    index: usize,
) -> Option<Rectangle<i32, Logical>> {
    let held = state.panes.get(pane)?;
    let outer = state.pane_outer(held).size;
    let drawing = crate::decoration::Drawing {
        rect: present::logical((0.0, 0.0), (f64::from(outer.w), f64::from(outer.h))),
        outer,
        alpha: 1.0,
        scale: 1.0,
    };
    held.decoration()?
        .layer_canvas(index, drawing)
        .map(|canvas| canvas.to_i32_round())
}

/// The one layer at `index` of a pane's style, its canvas's corner at `at`
/// in the target's physical pixels ([`layer_drawing_at`]):
/// `Decoration::layer_elements` at that layer's depth, keeping only what its
/// hook saw pushed for that layer ([`only_layer`]). A layer's self capture
/// (`offscreen::part_job`); `None` for a dormant layer, or one the pane's
/// style does not have. `tests::a_layers_capture_puts_its_canvas_corner_at_the_pad`,
/// `tests::one_layer_keeps_only_the_layer_it_names`.
pub(crate) fn one_layer(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    pane: crate::pane::PaneId,
    index: usize,
    at: Point<i32, Physical>,
    scale: f64,
) -> Option<Element> {
    let canvas = layer_canvas(state, pane, index)?;
    let held = state.panes.get(pane)?;
    let outer = state.pane_outer(held).size;
    let (_, depth, _) = held
        .decoration()?
        .layer_places()
        .find(|(each, ..)| *each == index)?;
    let drawing = layer_drawing_at(canvas, outer, at, scale);
    let title = state.pane_title(pane);
    let look = crate::decoration::Look {
        title: &title,
        focused: state.looks_focused(pane),
        pointer_inside: state.pointer_over(pane),
        caret: state.caret_in(pane),
        values: state.decorations.values(),
    };
    let decoration = state.panes.get_mut(pane)?.decoration_mut()?;
    let mut animating = false;
    let element = only_layer(index, |into, hook| {
        animating = decoration.layer_elements(renderer, depth, &look, drawing, into, hook);
    });
    // As `chrome` asks: the decoration has damaged nothing, so a frame not
    // asked for here never comes and its animation stops where it stood.
    if animating {
        state.redraw = true;
    }
    element
}

/// The drawing that puts a layer's `canvas` (its rectangle at rest, from
/// [`layer_canvas`]) with its corner at `at`, in physical pixels at `scale`:
/// the pane's own corner the layer's bleed inside it.
/// `tests::a_layers_capture_puts_its_canvas_corner_at_the_pad`.
pub(crate) fn layer_drawing_at(
    canvas: Rectangle<i32, Logical>,
    outer: Size<i32, Logical>,
    at: Point<i32, Physical>,
    scale: f64,
) -> crate::decoration::Drawing {
    let corner = at.to_f64().to_logical(scale);
    crate::decoration::Drawing {
        rect: present::logical(
            (
                corner.x - f64::from(canvas.loc.x),
                corner.y - f64::from(canvas.loc.y),
            ),
            (f64::from(outer.w), f64::from(outer.h)),
        ),
        outer,
        alpha: 1.0,
        scale,
    }
}

/// What `draw` pushes for the layer at `index` and nothing else: `draw` is
/// handed the list and `Decoration::layer_elements`' hook, which is called
/// before and after each layer it draws. `None` when that layer pushed
/// nothing or was not drawn. `tests::one_layer_keeps_only_the_layer_it_names`.
pub(crate) fn only_layer<T>(
    index: usize,
    draw: impl FnOnce(&mut Vec<T>, &mut dyn FnMut(&mut Vec<T>, usize, bool)),
) -> Option<T> {
    let mut list = Vec::new();
    let (mut start, mut kept) = (0, None);
    draw(&mut list, &mut |into: &mut Vec<T>, each, before| {
        if each == index {
            if before {
                start = into.len();
            } else {
                kept = Some(start..into.len());
            }
        }
    });
    list.drain(kept?).next()
}

/// The compositor's own scene for a pane, across the whole window.
///
/// The whole window and not the client's share of it, on purpose: while this is
/// what you are looking at there is no bar over it, and once there is, this is
/// drawn *over* the bar as it fades. Either way it covers the window, which is
/// what makes it the window rather than something sitting in one.
fn scene(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    elements: &mut Vec<Element>,
    pane: crate::pane::PaneId,
    frame: present::Frame,
    now: std::time::Duration,
    scale: f64,
) {
    let rect = frame.rect;
    let waited = state.panes.get(pane).map_or(0, |held| {
        i32::try_from(held.waited(now).as_millis()).unwrap_or(i32::MAX)
    });
    // Two fades, and they multiply. The scene's own, as the application it was
    // standing in for appears underneath it; and the pane's, because a window
    // being closed or animated by a script takes everything inside it along --
    // the scene is part of the window, not something laid over it.
    let fade = state.loading.fade;
    let alpha = frame.opacity
        * state
            .panes
            .get(pane)
            .map_or(1.0, |held| held.scene_alpha(now, fade));
    #[expect(clippy::cast_possible_truncation, reason = "a rect on this screen")]
    let area = smithay::utils::Rectangle::new(
        (rect.loc.x.round() as i32, rect.loc.y.round() as i32).into(),
        (
            (rect.size.w.round() as i32).max(1),
            (rect.size.h.round() as i32).max(1),
        )
            .into(),
    );
    let Some(held) = state.panes.get_mut(pane).and_then(Pane::scene_mut) else {
        return;
    };
    held.set_int("waited", waited);
    // Already an `Element`: a shell surface is a memory buffer on the software
    // path and a texture on the GPU one, and which of the two it is is its own
    // business rather than this function's.
    elements.extend(held.element(renderer, area, now, alpha, scale).element);
    // A scene animates on its own clock and damages nothing, so the next frame
    // has to be asked for or it stops where it stands -- mid-fade, most of all.
    //
    // Unconditional, and *not* `drawn.animating`, which is the honest answer to
    // a different question. The fade above is the compositor's own arithmetic
    // on `now` — `scene_alpha` — and it is applied to the element rather than
    // inside the scene, so Qt's flag knows nothing about it and would say
    // "nothing to draw" through the whole of it. A pane only has a scene while
    // it is waiting for its application or dissolving into one, so this asks
    // for frames for as long as that lasts and not a moment longer.
    state.redraw = true;
}

/// One picture to draw: which part of the desktop, at what scale, with or
/// without the pointer.
///
/// Bundled rather than passed as three arguments because there is now more than
/// one reason to ask for a picture. A screen capture wants the same frame a
/// monitor gets and usually *not* the pointer, and a fourth positional `bool`
/// on a call that already had five arguments is the kind of thing that gets
/// passed in the wrong order once and then silently stays wrong.
/// Named `Picture` and not `Frame` because `present::Frame` already means
/// something else in here — a window's transform for this instant. Two
/// `Frame`s in one file is a reader's problem for as long as the file lasts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Picture {
    /// Where this picture sits in the global space.
    pub(crate) screen: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    /// Device pixels per logical one.
    pub(crate) scale: f64,
    /// Whether to draw the pointer.
    pub(crate) cursor: bool,
}

impl Picture {
    /// What a monitor gets: its whole area, its own scale, pointer included.
    pub(crate) fn screen(
        screen: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
        scale: f64,
    ) -> Self {
        Self {
            screen,
            scale,
            cursor: true,
        }
    }
}

/// One pane's place in the draw walk: which pane, its client if it has one,
/// the slot it occupies, and how it is being drawn this instant.
///
/// An alias and not a struct: `by_depth` is generic over `(T, f32)`, so this
/// is the `T`, and naming it is what keeps the `Vec<(…, f32)>` below readable.
/// It buys no safety and is not here for any — the four fields are four
/// different types, so nothing in it can be transposed unnoticed.
type Node = (
    crate::pane::PaneId,
    Option<Window>,
    smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    present::Frame,
);

/// Order nodes for drawing: nearest the viewer first, equal depths as they
/// came.
///
/// Generic over what is carried so the compositor's walk and the tests drive
/// the same code — `Solium` is not constructible in a test, and an ordering
/// rule checked against a second copy of itself is not checked.
///
/// **Descending, because the list this orders is walked topmost-first.** A
/// higher `z` is nearer the viewer and nearer the viewer is earlier in
/// `elements`, so the comparison reads `right` against `left` rather than the
/// other way round. Getting it backwards does not look like a sort error: it
/// looks like raising a window hides it.
///
/// `sort_by` and not `sort_unstable_by`, because equal depths must keep the
/// order the stack gave them and every window is `0.0` — that is what makes
/// the default free. Worth recording that no test below rejects the unstable
/// one: on this toolchain the two agree for every arrangement of up to 32
/// nodes, so the difference is only reachable with a stack larger than anyone
/// looks at, and a fixture that size would be pinning `core`'s small-sort
/// threshold rather than this line. The stable sort is correct by the
/// function's own contract; the tests pin everything else about it.
///
/// `total_cmp` is deliberately not used. It orders NaN — above every finite
/// float — so a NaN depth reaching here would put a window at the front of the
/// stack for reasons nothing on screen explains. Answering `Equal` leaves it
/// where the stack put it, which is what the default depth does.
///
/// **This is not the guard, and it is not merely a tidying.** `script::depth_from`
/// refuses a non-finite depth at the boundary and is the only path from a
/// script to `Frame::z`, so nothing can reach this today — it is the layer
/// that must not make things worse if a second producer ever appears. And what
/// it prevents is not a scrambled stack: `Equal` against every finite depth,
/// while those do not tie with each other, is **not a total order**, and
/// `slice::sort_by` handed one *may panic* — on this walk, a dead compositor.
/// So if a second `Frame::z` producer is ever added, it needs its own refusal;
/// this line makes the failure survivable, not impossible.
fn by_depth<T>(nodes: &mut [(T, f32)]) {
    nodes.sort_by(|(_, left), (_, right)| {
        right.partial_cmp(left).unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Whether a pane that lives at `slot` is drawn on `screen` at all.
///
/// **The renderer's containment rule, written once.** [`elements`] asks it of
/// every pane for every screen before drawing anything -- the comment where it
/// does says why the slot and not the transform decides -- and [`prepare`]
/// asks it through `Solium::on_any_output` before capturing one. Exclusive, as
/// `Rectangle::overlaps` is: a slot whose right edge is a monitor's left edge
/// has no pixel on it.
///
/// **And every question about whether the user can see a window asks it
/// too**, which until #134's third review none did: `Solium::on_stage` of the
/// screens a window is headed for, and the hit tests of the screen under the
/// point. They asked where the frame was drawn against *every* screen, which
/// is the same question only while there is one. With two side by side, a
/// workspace parked a screen away is parked on the other monitor -- which does
/// not draw it -- so a window there counted as on stage, was handed the
/// keyboard and took the clicks meant for what the other monitor does draw.
/// `on_two_monitors_a_window_on_the_left_monitors_hidden_workspace_is_not_on_stage`
/// and `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws`
/// pin the two halves; `state::nothing_on_stage` and `state::shown_at` are
/// where they ask.
///
/// **This is one of the renderer's two culls, and only this one is shared.**
/// Within the monitors this allows, [`elements`] culls again by the pane's
/// *bleed* -- the reach of its widest decoration layer -- and never culls a
/// frame with a matrix or a deform, since nothing cheap bounds where those put
/// pixels. `state::nothing_on_stage` asks neither: it measures the frame's own
/// rectangle. So a glow reaching onto a screen from a window just off it, or a
/// tilted window whose rectangle is off it, is drawn where `on_stage` says
/// nothing is. Older than #134 and harmless so far -- a glow is nothing to hand
/// the keyboard to -- but the two halves are not the same rule, and a question
/// that needs the renderer's exact answer cannot borrow the focus one.
/// `the_focus_half_counts_a_frame_by_its_rectangle_where_the_renderer_counts_its_bleed_and_every_transform`
/// pins both as they stand.
pub(crate) fn drawn_on(slot: Rectangle<i32, Logical>, screen: Rectangle<i32, Logical>) -> bool {
    slot.overlaps(screen)
}

/// What to draw for one frame, topmost first.
///
/// Called once per monitor. `frame.screen` is where that output sits in the
/// global space, and every rect here is global — a pane's slot, the pointer,
/// the work area — so the last thing each of them does is move by
/// `-screen.loc`. Getting that offset wrong does not look like an offset: the
/// second monitor draws the first monitor's picture, which reads as mirroring.
///
/// Anything that does not touch this screen is left out entirely, so a window
/// on the other monitor costs this one nothing.
pub(crate) fn elements(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    prepared: &Prepared,
    picture: Picture,
) -> Vec<Element> {
    // The compositor's own share of a frame. Qt's share is taken out from
    // underneath this by the `Census` and `Qml` spans nested inside it, which
    // is the whole reason the phases are exclusive: a decoration that costs
    // 7 ms of Qt and a pane walk that costs 7 ms of Rust look identical from
    // out here and want completely different fixes.
    let _elements = crate::pacing::span(crate::pacing::Phase::Elements);
    let Picture {
        screen,
        scale,
        cursor: with_cursor,
    } = picture;
    let now = state.clock.now();
    let output_scale = Scale::from(scale);
    let mut elements = Vec::new();
    // From global coordinates into this output's own, as a float offset so a
    // window drawn mid-animation is not rounded to the monitor's grid.
    let shift = smithay::utils::Point::<f64, smithay::utils::Logical>::from((
        -f64::from(screen.loc.x),
        -f64::from(screen.loc.y),
    ));

    // The pointer, above everything — including anything a shell anchors on
    // top. Nothing else draws it, so leaving it out is not a missing detail:
    // it is a session where the mouse appears not to work.
    //
    // Left out of a capture unless it was asked for, which is what
    // `overlay_cursor` in the screencopy protocol means: a screenshot of a
    // window should not have somebody's mouse in it.
    if with_cursor {
        elements.extend(cursor(
            state,
            renderer,
            output_scale,
            scale,
            shift,
            screen.size,
        ));
    }

    // Locked, so this is the whole frame. Everything below here belongs to the
    // session -- windows, bars, the shell, the tweaks panel -- and none of it
    // is drawn. Returning early is the point: a filter further down is
    // something a later change can slip past, and what slips past is somebody
    // else's screen.
    //
    // The backdrop goes on unconditionally, underneath, even when the client
    // has a surface here. A lock surface that is translucent, smaller than the
    // monitor, or simply has not painted yet would otherwise leave the desktop
    // visible through the gap, and a gap is exactly what this protocol exists
    // to rule out.
    //
    // Note that this is also what a *capture* of a locked screen gets:
    // screencopy goes through the same function, so a client recording the
    // screen sees the lock screen and not what is behind it.
    if let Some(lock) = state.lock.as_ref() {
        if let Some(output) = state.output_for(screen)
            && let Some(surface) = lock.surface_for(&output)
        {
            elements.extend(
                render_elements_from_surface_tree::<
                    GlesRenderer,
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(
                    renderer,
                    surface.wl_surface(),
                    (0, 0),
                    output_scale,
                    1.0,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(Element::Window2),
            );
        }
        elements.push(Element::Solid(
            smithay::backend::renderer::element::solid::SolidColorRenderElement::new(
                lock.blank(),
                smithay::utils::Rectangle::from_size(screen.size).to_physical_precise_round(scale),
                CommitCounter::default(),
                crate::lock::BLANK,
                Kind::Unspecified,
            ),
        ));
        return elements;
    }

    // The drag icon: whatever a client attached to the drag it started, at the
    // pointer, above everything the session owns. Issue #57 is that this was
    // never drawn at all.
    //
    // **Here, and not beside the pointer above, because of the lock.** A drag
    // icon is a client's surface with a client's pixels in it, and
    // `ext-session-lock` exists to say that none of those reach a locked
    // screen. The early return a few lines up is that promise, and putting the
    // icon below it means the icon inherits it -- rather than a second `if
    // state.lock.is_some()` of its own, which is a thing a later change can
    // forget to update while the one above it goes on looking correct. The
    // pointer itself is above the return on purpose and is not the same case:
    // it is the compositor's own arrow or a theme's, and a lock screen you
    // cannot aim at is a lock screen you cannot type a password into.
    //
    // Everything after this point is the session -- scripted bars, anchored
    // layers, windows -- so the icon is above all of it and below only the
    // pointer, which is where the thing being dragged belongs: under the tip
    // of the cursor that is carrying it.
    //
    // Gated on `with_cursor` for the same reason the pointer is: a screencopy
    // that asked not to have the mouse in it did not ask to have half a drag
    // in it either.
    if with_cursor {
        elements.extend(drag_icon(state, renderer, output_scale, scale, shift));
    }

    // Everything else, in the one order there is: [`stacked`] lists it,
    // topmost first, and this turns each entry into elements and nothing more,
    // with the slots beside a surface when any slot is wanted ([`stacked_walk`]).
    let output = state.output_for(screen);
    let listed = stacked(state, screen, now);
    let mut draw = |each: Walked| {
        match each {
            Walked::Stacked(Stacked::Layer(surface, geometry)) => {
                // A layer map's geometry is already in its own output's
                // coordinates, so these are the one thing on this list that
                // must *not* be shifted.
                let origin = geometry.loc.to_physical_precise_round(scale);
                let layer_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    surface.render_elements(renderer, origin, output_scale, 1.0);
                elements.extend(layer_elements.into_iter().map(|element| {
                    Element::Window(RescaleRenderElement::from_element(
                        element,
                        origin,
                        Scale::from(1.0),
                    ))
                }));
            }
            Walked::Stacked(Stacked::Surface(id, area, alpha)) => {
                if let Some(output) = output.as_ref() {
                    elements.extend(scripted(
                        state, renderer, output, id, area, alpha, screen, now, scale,
                    ));
                }
            }
            Walked::Stacked(Stacked::Panes(nodes)) => {
                panes(
                    state,
                    renderer,
                    prepared,
                    &mut elements,
                    nodes,
                    screen,
                    now,
                    scale,
                );
            }
            Walked::Slot(owner, slot, rect, alpha) => {
                let rect = match owner {
                    crate::effect::plan::Owner::LayerShell(_) => rect,
                    crate::effect::plan::Owner::Surface(..)
                    | crate::effect::plan::Owner::Pane(..) => {
                        Rectangle::new(rect.loc - screen.loc, rect.size)
                    }
                };
                let part = Mask::Rect {
                    rect: rect.to_f64(),
                    radii: Corners::all(0.0),
                };
                elements.extend(slot_element(
                    state,
                    &prepared.slots,
                    &owner,
                    slot,
                    part,
                    scale,
                    alpha,
                ));
            }
        }
    };
    match output.as_ref().filter(|_| !prepared.slots.is_empty()) {
        Some(on) => stacked_walk(
            listed,
            &on.name(),
            &|owner, slot| prepared.slots.is_ready(owner, slot),
            &mut draw,
        ),
        None => listed.into_iter().map(Walked::Stacked).for_each(&mut draw),
    }

    elements
}

/// One entry of a monitor's frame, before it is turned into elements.
#[derive(Debug)]
pub(crate) enum Stacked {
    /// A client's layer surface, and where it is in its output's own
    /// coordinates.
    Layer(LayerSurface, Rectangle<i32, Logical>),
    /// A script's surface: which one, where on the desktop its selection has
    /// carried it, and how much of it the selection shows.
    Surface(crate::scripted::SurfaceId, Rectangle<i32, Logical>, f32),
    /// Panes, in the order they are drawn in.
    Panes(Vec<(Node, f32)>),
}

/// One entry of [`stacked_walk`]: an entry of [`stacked`], or an effect's
/// slot beside a layer surface or a scripted surface, with the rectangle and
/// alpha that surface is drawn with, as its [`Stacked`] carries them.
#[derive(Debug)]
pub(crate) enum Walked {
    Stacked(Stacked),
    Slot(
        crate::effect::plan::Owner,
        Slot,
        Rectangle<i32, Logical>,
        f32,
    ),
}

/// [`stacked`]'s entries with their slots (Ruling 15), `elements`' loop: a
/// layer surface's or a scripted surface's `front` just over it, its
/// `replace` in its place and its `behind` just under it, where `ready` says
/// a slot has a result. `output` names the output they are on, which a
/// scripted surface's slots are kept under.
/// `state::tests::real_client::reflow_on_close::stacking::a_layer_shell_behind_slot_is_drawn_just_under_its_surface_and_a_surface_front_slot_just_over_it`;
/// with no slot ready, every stacking test.
pub(crate) fn stacked_walk(
    stacked: Vec<Stacked>,
    output: &str,
    ready: &dyn Fn(&crate::effect::plan::Owner, Slot) -> bool,
    mut push: impl FnMut(Walked),
) {
    use smithay::reexports::wayland_server::Resource as _;
    for each in stacked {
        let (owner, rect, alpha) = match &each {
            Stacked::Layer(surface, geometry) => (
                crate::effect::plan::Owner::LayerShell(surface.wl_surface().id()),
                *geometry,
                1.0,
            ),
            Stacked::Surface(id, area, alpha) => (
                crate::effect::plan::Owner::Surface(*id, output.to_owned()),
                *area,
                *alpha,
            ),
            Stacked::Panes(_) => {
                push(Walked::Stacked(each));
                continue;
            }
        };
        let mut each = Some(each);
        bracket(
            |slot| ready(&owner, slot),
            |slot| match slot {
                Some(slot) => push(Walked::Slot(owner.clone(), slot, rect, alpha)),
                None => {
                    if let Some(each) = each.take() {
                        push(Walked::Stacked(each));
                    }
                }
            },
        );
    }
}

/// A part with its slots, topmost first: its `front`, then its `replace` in
/// its place or the part itself (`None`), then its `behind`, each slot only
/// where `has` says. A layer surface, a scripted surface and a window's
/// popups are walked so. `tests::a_part_is_bracketed_by_its_front_and_behind_and_replaced_in_its_place`.
pub(crate) fn bracket(has: impl Fn(Slot) -> bool, mut piece: impl FnMut(Option<Slot>)) {
    if has(Slot::Front) {
        piece(Some(Slot::Front));
    }
    piece(has(Slot::Replace).then_some(Slot::Replace));
    if has(Slot::Behind) {
        piece(Some(Slot::Behind));
    }
}

/// Everything on one monitor below the drag icon, topmost first.
///
/// What [`elements`] draws there and all it draws, so a test can ask what is
/// over what without a GPU. The bands are in `crate::stack`'s order, which the
/// hit tests read as well; within a band, a client's layer surfaces are in
/// `layer::on`'s order, the last mapped on top, a script's are first declared
/// on top, and the panes are [`by_depth`]'s.
/// `state::tests`' stacking tests ask it.
pub(crate) fn stacked(
    state: &Solium,
    screen: Rectangle<i32, Logical>,
    now: std::time::Duration,
) -> Vec<Stacked> {
    let output = state.output_for(screen);

    // **Depth orders this walk and nothing else.** `z` is a sort key over a
    // painter's-algorithm list, not a coordinate: it decides which window
    // covers which, and `rect` stays the truth for input, so a window raised
    // above its neighbour is still clicked where the layout put it. See the
    // spec's *Hit-testing does not move*.
    //
    // **The two cheap-path gates inside [`panes`]' loop are right not to ask
    // about `z`, and this sort is the reason.** The off-screen cull and the
    // warp test only `matrix` and `deform`, so a flat window carrying a depth
    // goes down the flat path — but the reordering has already happened by
    // then, out here, where every pane passes through it whichever path it
    // goes on to take. A depth needs no texture, no mesh and no program to be
    // honoured; it is spent entirely on this line. **A flat window raised
    // above its neighbours is reordered and still drawn from its own
    // surfaces**, which is the point: raising a window must not cost it an
    // offscreen render.
    //
    // The third gate is *not* in [`panes`] and that argument does not cover
    // it. It is in `prepare`, which walks `on_screen()` separately and
    // earlier, and its panes never see this sort. It is safe for an unrelated
    // reason: `Prepared::warp` and `Prepared::over` find their answer *by
    // `Window`*, so what `prepare` produces is content-addressed and the order
    // it produced it in cannot reach here. Left in stacking order deliberately
    // — sorting it would be a sort per frame buying nothing.
    //
    // The frame is resolved here and carried rather than resolved again in the
    // loop, and resolved at **the `now` this frame already sampled** rather
    // than from the clock. `present::Clock` reads through instead of caching,
    // so `Solium::drawn` samples once per call: resolving twice would sort on
    // one instant and draw on another, and resolving per pane would shear each
    // window against its neighbours and against the scripted layers above and
    // below, which are all placed at `now`. One instant for everything on this
    // screen, and one `drawn` per pane per screen.
    //
    // That is slightly *more* than the loop cost before there was a sort: the
    // old walk resolved a frame only after the `ours && !pane_has_scene`
    // continue, so a pane on its way out cost nothing. `drawn_at` is
    // side-effect-free, so the extra call is wasted work rather than a
    // behaviour change — but it is not free when a group names a monitor,
    // where it searches the outputs per pane. The other new cost is one
    // `Vec<(Node, f32)>` per screen per frame, about 180 bytes a pane.
    //
    // The transform inside it is expressed against the *outer* rect — the
    // window including its frame — so the frame scales and moves with the
    // window rather than beside it. Computed in global coordinates, because
    // that is the space a script's target was written in, and moved onto each
    // screen afterwards.
    let mut order: Vec<(Node, f32)> = state
        .on_screen()
        .into_iter()
        .filter_map(|(pane, window)| {
            let global = state.pane_outer_of(pane)?;
            let frame = state.drawn_id_at(pane, global, now);
            Some(((pane, window, global, frame), frame.z))
        })
        .collect();
    by_depth(&mut order);

    // The window lifted over the bars is taken out of the windows' band and
    // drawn in its own, popups and all; see `crate::stack::lifted`.
    let lifted = state.lifted_on(screen);
    let (front, rest): (Vec<_>, Vec<_>) = order
        .into_iter()
        .partition(|((pane, ..), _)| Some(*pane) == lifted);
    let (mut front, mut rest) = (Some(front), Some(rest));

    let mut drawn = Vec::new();
    for band in crate::stack::order(lifted.is_some()) {
        match band {
            Band::Layer(layer, Owner::Client) => {
                if let Some(output) = output.as_ref() {
                    let map = layer_map_for_output(output);
                    drawn.extend(layer::on(&map, layer).filter_map(|surface| {
                        Some(Stacked::Layer(
                            surface.clone(),
                            map.layer_geometry(surface)?,
                        ))
                    }));
                }
            }
            Band::Layer(layer, Owner::Script) => drawn.extend(wanted(state, layer, screen)),
            Band::Fullscreen => drawn.extend(front.take().map(Stacked::Panes)),
            Band::Windows => drawn.extend(rest.take().map(Stacked::Panes)),
        }
    }
    drawn
}

/// A warp's mesh on one screen, over `part` of the pane's unit square: the
/// frame and the grid a geometry placed, both in global space, moved onto the
/// screen together; with no grid, the part where the frame puts it.
/// `tests::a_genie_on_the_second_monitor_lands_on_its_target`,
/// `tests::a_genie_on_the_first_monitor_is_unchanged`.
pub(crate) fn warp_mesh_on(
    screen: Rectangle<i32, Logical>,
    frame: &present::Frame,
    grid: Option<&crate::warp::Grid>,
    part: crate::warp::UnitRect,
    scale: f64,
) -> Option<crate::warp::Mesh> {
    let shift = Point::<f64, Logical>::from((-f64::from(screen.loc.x), -f64::from(screen.loc.y)));
    let rect = Rectangle::new(frame.rect.loc + shift, frame.rect.size);
    match grid {
        Some(grid) => crate::warp::mesh_grid(grid, shift, rect, frame.matrix, frame.pivot, scale),
        None => crate::warp::mesh_part(rect, part, frame.matrix, frame.pivot, scale),
    }
}

/// Draw panes, in the order given.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame's own state, handed down from `elements`"
)]
fn panes(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    prepared: &Prepared,
    elements: &mut Vec<Element>,
    order: Vec<(Node, f32)>,
    screen: Rectangle<i32, Logical>,
    now: std::time::Duration,
    scale: f64,
) {
    let output_scale = Scale::from(scale);
    // `elements`' offset from global coordinates into this output's own.
    let shift = smithay::utils::Point::<f64, smithay::utils::Logical>::from((
        -f64::from(screen.loc.x),
        -f64::from(screen.loc.y),
    ));
    let onto = |rect: smithay::utils::Rectangle<f64, smithay::utils::Logical>| {
        smithay::utils::Rectangle::new(rect.loc + shift, rect.size)
    };

    for ((pane, window, global, mut frame), _) in order {
        let outer = smithay::utils::Rectangle::new(global.loc - screen.loc, global.size);
        // Whether what is drawn here is ours or the client's. A window whose
        // application has not arrived is obviously ours; so is one whose
        // client has mapped and is not ready to be seen, which is most of the
        // moment after an application starts. The scene covers both, and
        // covering the second is what makes the handover a handover rather
        // than a gap with a window either side of it.
        let ours = !window
            .as_ref()
            .is_some_and(|window| state.client_ready(window));
        // The remains of a window whose client has gone: drawn from its
        // picture, or from a fill if it left none, below. One that kept the
        // scene standing in for it is `ours` with a scene, and the ordinary
        // walk for one of those draws it.
        let remains = state
            .panes
            .get(pane)
            .is_some_and(|held| held.ghost() && !held.has_scene());
        // Nothing of the client's left to draw and nothing of ours either --
        // a client that has mapped and not painted, or that unmapped itself --
        // so do not draw half of it.
        if ours && !state.pane_has_scene(pane) && !remains {
            continue;
        }

        // **A window is drawn only on the monitors it lives on.** Its slot
        // decides that, not its transform.
        //
        // This is the rule that makes a mode work on more than one screen, and
        // it is not an optimisation. A workspace switch does not move windows:
        // it draws the ones belonging to other workspaces a screen away. With
        // one monitor, "a screen away" means off the desktop, which is how they
        // are hidden. With two side by side, one screen away is *the other
        // monitor* — so switching the left screen's workspace threw its
        // windows onto the right screen, on top of whatever was there.
        //
        // The intent was always containment; a single screen just made moving
        // and hiding the same operation. Deciding by the slot restores it on
        // any number of monitors, and keeps the slide one screen long, which is
        // what makes it read as a slide. Offsetting by the whole desk instead
        // would hide them correctly and look wrong: the window would leave the
        // screen halfway through and the next would arrive halfway through,
        // with empty screen in between.
        //
        // The *slot* and not the drawn rect, so a window straddling the bezel
        // — dragged between screens, where the slot itself is on both — is
        // still drawn on both. A transform can move a window around its own
        // monitors and off them. It cannot put it on somebody else's.
        //
        // Through [`drawn_on`], which is this rule and is also what every
        // question about whether the user can see a window asks.
        if !drawn_on(global, screen) {
            continue;
        }
        // And within its own monitors, one that has been transformed clean off
        // this screen has nothing to contribute to it.
        //
        // Skipped only when the transform is a plain rectangle. A matrix or a
        // deform can put pixels well outside `frame.rect` — a genie reaches
        // toward a dock — and there is no cheap rect that bounds it, so those
        // are always drawn and the renderer clips.
        //
        // **Tested against the bleed and not against the pane**, which is the
        // one place a pane's own rectangle stood between a layer and the
        // screen. A style that reaches 200px past its window has 199 of them
        // still on this monitor when the window itself is one pixel off the
        // edge, and culling by the window would have made a glow vanish the
        // instant the thing it belongs to left — during a workspace slide,
        // which is exactly when it is being looked at.
        //
        // Deliberately *not* the check above. That one is the slot, and it is
        // containment: a window parked a screen away to hide it must not throw
        // a glow onto the screen you are looking at, however far it bleeds.
        //
        // Through the same `spread` the layers themselves are placed by, so the
        // rectangle this keeps a pane alive for is the rectangle its widest
        // layer will actually occupy — including the scaling, since a window
        // enlarged by a mode has its bleed enlarged with it. And grown by the
        // furthest its effects' results reach
        // (`tests::the_bleed_cull_counts_an_effects_reach`).
        let bled = crate::decoration::spread(
            crate::decoration::Drawing {
                rect: frame.rect,
                outer: outer.size,
                alpha: frame.opacity,
                scale,
            },
            state
                .panes
                .get(pane)
                .and_then(Pane::decoration)
                .map_or_else(
                    crate::style::Bleed::default,
                    crate::decoration::Decoration::bleed,
                ),
        )
        .drawn;
        if frame.matrix.is_identity()
            && frame.deform.is_none()
            && !reaches(bled, prepared.slots.reach(pane), screen)
        {
            continue;
        }
        let drawn_global = frame;
        frame.rect = onto(frame.rect);
        // Where this pane's layers go, computed once for all three depths.
        let drawing = crate::decoration::Drawing {
            rect: frame.rect,
            outer: outer.size,
            alpha: frame.opacity,
            scale,
        };

        // Its client has gone: its frame's layers, fading with the rest of it,
        // round what the client left. In `PANE_ORDER`, like a live client.
        if remains {
            let mut client = Some(remains_elements(state, pane, &frame, outer.size, scale));
            pane_pieces(elements, |elements, piece| match piece {
                Piece::Layers(depth) => {
                    chrome(state, renderer, elements, None, pane, depth, drawing);
                }
                Piece::Client => elements.extend(client.take().into_iter().flatten()),
                Piece::Slot(..) => {}
            });
            continue;
        }

        // Nothing of the application to draw yet, so this window is entirely
        // ours and the scene has all of it, bar included. The frame is built
        // and its room reserved — which is why the window does not change shape
        // when the application arrives — but drawing a bar over a surface that
        // already carries the name says it twice, so that is a setting and it
        // is off.
        if ours {
            // Through `PANE_ORDER` as well, so a pane waiting for its
            // application is layered the same way it will be once it arrives:
            // an `above` layer covers the standing-in scene exactly as it will
            // cover the client, and the handover does not restack anything.
            pane_pieces(elements, |elements, piece| match piece {
                Piece::Layers(depth) if state.loading.decorated => {
                    chrome(state, renderer, elements, None, pane, depth, drawing);
                }
                Piece::Layers(_) | Piece::Slot(..) => (),
                Piece::Client => scene(state, renderer, elements, pane, frame, now, scale),
            });
            continue;
        }

        let Some(window) = window else {
            continue;
        };
        // A refused present geometry under `failed = "hide"`: nothing of it
        // this pass (`tests::a_refused_present_follows_its_failed_policy`).
        if prepared.hidden(&window) {
            continue;
        }

        // The application has painted and the scene is fading off it. Pushed
        // before the frame and before the client, so it is above both: what is
        // underneath is already the window, and this dissolves to reveal it
        // rather than being swapped for it.
        if state.pane_has_scene(pane) {
            scene(state, renderer, elements, pane, frame, now, scale);
        }

        let Some(real) = state.real_geometry(&window) else {
            continue;
        };

        // A transform that is not identity cannot be drawn as a rectangle. The
        // window is rendered flat into a texture first — frame included — and
        // that texture is bent, so the whole window deforms as one thing
        // instead of the client tilting away from its own titlebar. Its popups
        // are not in that texture (`state::tests::a_warped_panes_capture_holds_no_popups`):
        // they are a capture of their own, bent by the same matrix and deform
        // over their part of the pane, and drawn in front of it
        // (`tests::a_warped_panes_popups_are_in_front_of_it`).
        //
        // The deform's anchor is resolved *here*, on the frame that draws it,
        // because what it is aimed at moves — see `present::Anchor`. An anchor
        // that resolves to nothing leaves `aimed` empty, and a window with no
        // matrix then takes the flat path below as if it had never asked for
        // an effect.
        // Its grid is the one `prepare` built this pass, once for every
        // output (`present_grid`).
        let aimed = state.aimed_at_for(pane, frame.deform);
        if (!frame.matrix.is_identity() || aimed.is_some())
            && let Some(warped) = prepared.warp(&window)
            && let Some(pane_mesh) = warp_mesh_on(
                screen,
                &drawn_global,
                warped.grid.as_ref(),
                crate::warp::UnitRect::WHOLE,
                scale,
            )
        {
            let (texture, program, id, commit) = (
                warped.texture.clone(),
                warped.program,
                warped.id.clone(),
                warped.commit,
            );
            // The pane's own mesh first: it can fail (a vertex behind the
            // viewer), and then nothing of the warp is pushed and the pane
            // falls through to the flat path below, as before. A popups' mesh
            // that fails alone drops the popups for that frame and keeps the
            // pane. Popups first, nearer the front:
            // `tests::a_warped_panes_popups_are_in_front_of_it`, and
            // `dev/present-check.sh`'s `menu` case through the real draw.
            let mut pane_warp = Some((id, texture, pane_mesh));
            for piece in WARP_ORDER {
                match piece {
                    WarpPiece::Over => {
                        if let Some(over) = prepared.over(&window)
                            && let Some(mesh) = warp_mesh_on(
                                screen,
                                &drawn_global,
                                over.grid.as_ref(),
                                over.part,
                                scale,
                            )
                        {
                            elements.push(Element::Warped(crate::warp::Warp::new(
                                over.id.clone(),
                                over.commit,
                                over.texture.clone(),
                                mesh,
                                frame.opacity,
                                over.program,
                            )));
                        }
                    }
                    WarpPiece::Pane => {
                        // Its pane capture's id for life, and the commit
                        // `prepare` moved only if the picture or the mesh
                        // changed, so a still warp is not damaged and a moving
                        // one is: `keyed::tests::a_warp_at_rest_keeps_its_commit`.
                        if let Some((id, texture, mesh)) = pane_warp.take() {
                            elements.push(Element::Warped(crate::warp::Warp::new(
                                id,
                                commit,
                                texture,
                                mesh,
                                frame.opacity,
                                program,
                            )));
                        }
                    }
                }
            }
            continue;
        }

        // The surface tree is built as if at its real size and then scaled,
        // which keeps subsurface offsets correct for free.
        //
        // **The same arithmetic bridges a live resize**, and that is the point
        // rather than a coincidence: this factor is 1 in ordinary use only
        // because the drawn rectangle is derived from the client's own size.
        // Give the pane a rectangle the client has not agreed to yet -- which
        // is what `pane_geometry` does while an edge is being dragged -- and
        // this stretches the buffer the client last painted to fill it, with no
        // second scaling path and nothing new in the element list. When the
        // client commits the size it was asked for the two sizes are equal
        // again and the window is pixel-exact. See `crate::resizing`.
        //
        // **And a tiled client that committed more than its tile is cut to
        // it, not squashed into it (#133).** `pane_geometry` caps the pane at
        // the tile, so the client rectangle is the tile's share; dividing that
        // by the committed size would shrink the whole buffer into the tile,
        // which turns the spill over the neighbour into a squash. So the
        // buffer is drawn at its own size -- times the frame's zoom, for an
        // open, a close or a thumbnail -- and cut to the client rectangle.
        // [`place_client`] is all of it, and it is shared with the hit test so
        // a press lands on the pixel the picture put there.
        let fill = state.resize_fill(pane);
        let Some(placed) = state
            .panes
            .get(pane)
            .map(|held| place_client(state, held, &frame, outer.size, real.size))
        else {
            continue;
        };
        let (client, fitting) = (placed.client, placed.fit);
        let (across, down) = (fitting.factor.x, fitting.factor.y);

        // The other half of the resize trace: `state.rs` records what the layout
        // wrote and what the client was told, and this records what was actually
        // drawn. Together they answer the question a report of "it still
        // stutters" cannot — whether the frame drawn on a given frame came from
        // the slot or from the client's last commit. See `resizing::trace`.
        //
        // **This line and not the `layout` one is what every frame has.**
        // `move_pane` is reached only from a frame that carried a motion, so a
        // paused drag writes no `layout` line at all — which is precisely the
        // stretch of a gesture the trailing flush is about, and precisely when
        // a reader needs to know what the pane was drawn at and what its client
        // had. So the slot is repeated here rather than left to be joined
        // against a line that may not exist, and it is the *client* rectangle
        // for the same reason `state.rs` logs that one: `frame` is the outer
        // rectangle, a titlebar taller, and two rectangles that differ by a
        // decoration are two rectangles a reader subtracts by hand and gets
        // wrong.
        if crate::resizing::trace::on() {
            let slot = state.panes.get(pane).map_or(real, Pane::slot);
            crate::resizing::trace::line(
                "drawn",
                format_args!(
                    "pane={} frame={},{} {}x{} slot={},{} {}x{} committed={}x{} \
                     factor={across:.4},{down:.4} fill={fill:?} held={}",
                    pane.get(),
                    frame.rect.loc.x,
                    frame.rect.loc.y,
                    frame.rect.size.w,
                    frame.rect.size.h,
                    slot.loc.x,
                    slot.loc.y,
                    slot.size.w,
                    slot.size.h,
                    real.size.w,
                    real.size.h,
                    u8::from(state.holding_resize(pane)),
                ),
            );
        }

        let corner = client.loc.to_physical_precise_round(scale);
        // Where the buffer's corner goes, which a held picture's slack can move
        // off the client's. See [`place_client`].
        let origin = placed.origin.to_physical_precise_round(scale);

        // Popups go in ahead of the sandwich, which means above all of it.
        //
        // They used to be emitted inside `Piece::Client`, under a comment
        // saying they are above the window they belong to. True of the client
        // and false of the frame: `PANE_ORDER` puts `Above` and `Frame` in the
        // list first, earlier is nearer the front, so the titlebar was already
        // there and drew over them. A Firefox menu opened near the top of its
        // window came out sliced in half by its own titlebar.
        //
        // Ahead of `Above` too, not merely ahead of `Frame`. A menu is the
        // frontmost thing its window owns while it is up -- that is what a
        // grab means -- and a style's overlay layer covering one would be the
        // same bug with a different layer's name on it.
        //
        // In front on both paths: a warped pane's popups are a capture of
        // their own, drawn as a warp in front of the pane's above
        // (`tests::a_warped_panes_popups_are_in_front_of_it`).
        let (popups, covered) =
            popup_elements(renderer, &window, origin, output_scale, frame.opacity);
        let popups_part = covered.map(|covered| Mask::Rect {
            rect: Rectangle::new(
                (
                    placed.origin.x + f64::from(covered.loc.x) * fitting.factor.x,
                    placed.origin.y + f64::from(covered.loc.y) * fitting.factor.y,
                )
                    .into(),
                (
                    f64::from(covered.size.w) * fitting.factor.x,
                    f64::from(covered.size.h) * fitting.factor.y,
                )
                    .into(),
            ),
            radii: Corners::all(0.0),
        });
        let popups_owner = crate::effect::plan::Owner::Pane(pane, PaneSlot::Popups);
        let mut popups = Some(popups);
        // With their slots around them, as any part's (`bracket`).
        bracket(
            |slot| prepared.slots.is_ready(&popups_owner, slot),
            |slot| match slot {
                Some(slot) => elements.extend(popups_part.and_then(|part| {
                    slot_element(
                        state,
                        &prepared.slots,
                        &popups_owner,
                        slot,
                        part,
                        scale,
                        frame.opacity,
                    )
                })),
                // Scaled with the window and not cut to its tile: a menu has
                // to reach past its parent's tile, and a menu cut to it would
                // lose every item past the tile's edge. The window's own fit
                // with the cut taken off, which is
                // `a_popup_reaches_past_its_parents_tile`. This is the only
                // place a toplevel's popups are drawn on this path -- the
                // toplevel itself is drawn from its own surface tree, which
                // holds no popups; see [`toplevel_elements`].
                None => {
                    elements.extend(popups.take().into_iter().flatten().filter_map(|element| {
                        fitted(element, origin, fitting.uncut(), output_scale)
                            .map(Fitted::into_element)
                    }))
                }
            },
        );

        // **This is the sandwich.** The client goes into the list between the
        // layers its own style produced -- `above` and `frame` are already in
        // by the time `Piece::Client` comes round, and `behind` follows it --
        // which is the thing a single decoration file cannot express. The order
        // is `PANE_ORDER`'s and is not restated here.
        //
        // A layer covers the whole window rather than a strip of it: whatever
        // it does not draw on is left transparent, which is what lets a
        // decoration put its bar on any side, or draw a border, or both. Drawn
        // whenever there is a decoration at all, not only when it reserved
        // space: a frame that takes nothing and floats over the window -- a bar
        // that appears on hover, a border that does not push the client around
        // -- is a decoration too.
        //
        // A style's rounding and its programs, asked once here, outside the
        // walk, which holds the state: `None` for every unstyled window.
        let rounded = clipped(state, pane);
        // With its slots: what is ready this pass goes around the pieces it
        // belongs to, and with none the walk is `PANE_ORDER`
        // (`tests::with_no_slots_the_pane_walk_is_pane_order`).
        let ready = slot_ready(&prepared.slots, pane);
        let zoom = frame.rect.size.w / f64::from(outer.size.w.max(1));
        pane_walk(elements, &ready, |elements, piece| match piece {
            Piece::Layers(depth) => chrome(
                state,
                renderer,
                elements,
                Some(&prepared.slots),
                pane,
                depth,
                drawing,
            ),
            Piece::Slot(part, slot) => {
                let radii = declared_rounding(state, pane).map(|effect| effect.radii());
                let mask = match part {
                    PaneSlot::Client => crate::effect::mask::client_mask(
                        client,
                        radii.map(|radii| corners_times(radii, fitting.factor.x)),
                    ),
                    _ => crate::effect::mask::pane_mask(
                        frame.rect,
                        radii.map(|radii| corners_times(radii, zoom)),
                    ),
                };
                elements.extend(slot_element(
                    state,
                    &prepared.slots,
                    &crate::effect::plan::Owner::Pane(pane, part),
                    slot,
                    mask,
                    scale,
                    frame.opacity,
                ));
            }
            Piece::Client => {
                // Popups are not here: they went in above the whole sandwich,
                // before this walk started. See the comment there.

                // A surface's top-left is not the window's -- see
                // `window_surface_origin`. That is what made Firefox look
                // both misplaced and shadowed. Popups already did this;
                // toplevels did not.
                let surface_origin = window_surface_origin(origin, &window, scale);
                let window_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    toplevel_elements(
                        renderer,
                        &window,
                        surface_origin,
                        output_scale,
                        frame.opacity,
                    );
                // A style's rounding: each surface wrapped, in its own place,
                // and cut to the client's rectangle as drawn, taken back
                // through the zoom into the surfaces' own pixels
                // (`clip::tests::input_to_geo_maps_each_corner_of_a_surface_onto_the_client`,
                // `tests::a_zoomed_clients_clip_is_the_whole_client`). `None`
                // for every unstyled window, which takes the lines after this
                // unchanged.
                if let Some((effect, programs)) = &rounded {
                    let clip = drawn_clip(
                        Rectangle::new(corner, client.size.to_physical_precise_round(scale)),
                        origin,
                        fitting.factor,
                        crate::pass::physical_radii(*effect, scale),
                    );
                    elements.extend(window_elements.into_iter().filter_map(|element| {
                        fitted(
                            crate::clip::Clipped::new(element, clip, programs.clone()),
                            origin,
                            fitting,
                            output_scale,
                        )
                        .map(Fitted::into_clipped)
                    }));
                    return;
                }
                // Cut to the tile when there is anything to cut, and a surface
                // the cut leaves nothing of -- a subsurface wholly past the
                // tile's edge -- is dropped: `CropRenderElement` has no empty
                // element to hand back, and says so with `None`.
                elements.extend(window_elements.into_iter().filter_map(|element| {
                    fitted(element, origin, fitting, output_scale).map(Fitted::into_element)
                }));
            }
        });
    }
}

/// Everything a script asked the compositor to draw at one layer on this
/// screen, as [`stacked`] lists it.
///
/// One function for the wallpaper, a bar, an overlay and whatever else gets
/// declared — which is the whole point of `scripted.rs`. Nothing in here knows
/// what any of them are for.
fn wanted(
    state: &Solium,
    layer: crate::scripted::Layer,
    screen: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
) -> Vec<Stacked> {
    let Some(output) = state.output_for(screen) else {
        return Vec::new();
    };
    let Some(geometry) = state.space.output_geometry(&output) else {
        return Vec::new();
    };
    let primary = state.primary_output();

    // Which of them belong on this screen, decided before anything is
    // borrowed mutably to rasterise it.
    //
    // **Carried before culled.** A surface's declared placement says where it
    // lives; the selection it is in says where it is drawn, and a wallpaper
    // belonging to a workspace a screen away is declared on this monitor and
    // drawn nowhere near it. Culling on the declared rectangle would rasterise
    // a full-screen scene per desk, every frame, for pictures nobody can see.
    // So the order here is load-bearing: place, carry, cull, and only then ask
    // for a rasterisation.
    state
        .surfaces
        .iter()
        .filter(|surface| surface.layer() == layer)
        .filter_map(|surface| {
            let id = surface.id();
            let area = state.carried(
                id,
                &output,
                surface.area_on(&output, geometry, primary.as_ref())?,
            );
            area.overlaps(screen)
                .then(|| Stacked::Surface(id, area, state.carried_alpha(id, &output)))
        })
        .collect()
}

/// One scripted surface [`wanted`] listed, rasterised.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame's own state, handed down from `elements`"
)]
fn scripted(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    output: &smithay::output::Output,
    id: crate::scripted::SurfaceId,
    area: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    alpha: f32,
    screen: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    now: std::time::Duration,
    scale: f64,
) -> Vec<Element> {
    let Some(surface) = state.surfaces.get_mut(id) else {
        return Vec::new();
    };
    let Some(instance) = surface.instance_mut(output) else {
        return Vec::new();
    };
    let painted = instance.element(
        renderer,
        smithay::utils::Rectangle::new(area.loc - screen.loc, area.size),
        now,
        alpha,
        scale,
    );
    // Ask for another frame while it is still moving, exactly as `chrome`
    // does for a window frame.
    //
    // This did not used to be asked at all, which is the same defect one step
    // further on: a scripted surface got the next frame only when something
    // unrelated damaged the screen. A bar whose clock ticks on a `Timer` is
    // the plain case. The Timer fires between frames, through `qml::wake`, and
    // asks for the frame its change needs; this keeps asking while the change
    // animates
    // (`qml::wake::tests::a_clock_scene_repaints_once_a_second_with_no_other_damage`).
    if painted.animating {
        state.redraw = true;
    }
    painted.element.into_iter().collect()
}

/// Where a scripted surface's instance on `output` goes, if it goes there.
/// What its self capture holds.
/// `state::tests::real_client::every_wanted_slot_has_its_part_box_padded_by_its_reach`.
pub(crate) fn instance_area(
    state: &Solium,
    id: crate::scripted::SurfaceId,
    output: &smithay::output::Output,
) -> Option<Rectangle<i32, Logical>> {
    let geometry = state.space.output_geometry(output)?;
    let primary = state.primary_output();
    state
        .surfaces
        .get(id)?
        .area_on(output, geometry, primary.as_ref())
}

/// A scripted surface's instance on `output`, its corner at `at` in the
/// target's physical pixels, rounded to the logical pixel a scene is placed
/// on: its self capture (`offscreen::part_job`). `None` with no instance
/// there or nothing drawn.
pub(crate) fn scripted_instance(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    id: crate::scripted::SurfaceId,
    output: &smithay::output::Output,
    at: Point<i32, Physical>,
    scale: f64,
) -> Option<Element> {
    let area = instance_area(state, id, output)?;
    let now = state.clock.now();
    let corner = at.to_f64().to_logical(scale).to_i32_round();
    let painted = state.surfaces.get_mut(id)?.instance_mut(output)?.element(
        renderer,
        Rectangle::new(corner, area.size),
        now,
        1.0,
        scale,
    );
    // Still moving: the next frame is asked for, as `scripted` asks.
    if painted.animating {
        state.redraw = true;
    }
    painted.element
}

/// A client layer surface's rectangle on the output whose layer map holds
/// it, in that output's coordinates. What its self capture holds.
/// `state::tests::real_client::every_wanted_slot_has_its_part_box_padded_by_its_reach`.
pub(crate) fn layer_shell_geometry(
    state: &Solium,
    surface: &LayerSurface,
) -> Option<Rectangle<i32, Logical>> {
    state
        .space
        .outputs()
        .find_map(|output| layer_map_for_output(output).layer_geometry(surface))
}

/// Where *in the image* the pointer actually points, as the client set it.
///
/// Zero for a surface no client ever passed to `wl_pointer.set_cursor`, which
/// is the only thing that fills this in — checked against
/// `wayland/seat/pointer.rs`, where `CursorImageAttributes` is inserted. A drag
/// icon is therefore always zero here today, and the call is still made rather
/// than skipped: see [`drag_icon`], which explains what would have to change
/// for it not to be.
fn hotspot(surface: &WlSurface) -> smithay::utils::Point<i32, smithay::utils::Logical> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<Mutex<CursorImageAttributes>>()
            .and_then(|attributes| attributes.lock().ok())
            .map(|attributes| attributes.hotspot)
            .unwrap_or_default()
    })
}

/// Where a picture carried by the pointer has its top-left corner, in the
/// output's own logical coordinates.
///
/// **The subtraction is the content, and its sign is the whole of it.** The
/// hotspot is a point measured *inside* the image — the tip of an arrow, the
/// middle of a crosshair — so the image's corner has to go that far up and
/// left of where the pointer is for that point to land on the pointer.
/// Adding instead puts an I-beam's tip a few pixels off the text it is meant
/// to be between, and puts a crosshair's centre a whole image away from what
/// is being aimed at.
///
/// Pure, and taking the two points already fetched, so the rule can be pinned:
/// the alternative is a `Solium` and a `GlesRenderer`, neither of which a unit
/// test in this crate can build, and a sign that is only checked by running
/// the compositor is a sign that is checked by somebody noticing.
fn origin_at(
    pointer: smithay::utils::Point<f64, smithay::utils::Logical>,
    hotspot: smithay::utils::Point<i32, smithay::utils::Logical>,
) -> smithay::utils::Point<i32, smithay::utils::Logical> {
    pointer.to_i32_round() - hotspot
}

/// The surface a client attached to the drag it is running, at the pointer.
///
/// **Issue #57: nothing drew this.** The data reached the other window and the
/// drop landed, so a drag between two windows worked — invisibly, for its whole
/// length, which is indistinguishable from one that failed and is why people
/// let go over the wrong window. See `Solium::dnd_icon` for why the surface has
/// to be kept when the drag starts rather than asked for here.
///
/// Drawn through `Kind::Unspecified` rather than `Kind::Cursor`, and that is
/// not cosmetic: `Kind::Cursor` is what offers an element to the DRM cursor
/// plane, there is one such plane, and the pointer itself is already on it.
/// Marking the icon as well would have two elements competing for one plane
/// every frame of every drag — at best the icon is composited anyway, at worst
/// it takes the plane and the pointer is the thing that disappears.
///
/// **A client that positions its icon with `wl_surface.offset` is not honoured
/// yet, and that is a known limit.** The accumulated delta lives in
/// `SurfaceAttributes::buffer_delta`, and nothing in smithay's renderer reads
/// it — `grep -rn buffer_delta` over 0.7's `src` finds it only in
/// `wayland/compositor`, never in `backend/renderer`, so
/// `render_elements_from_surface_tree` places the root at exactly the origin it
/// is given. Toolkits use that request to line the icon's grab point up with
/// the cursor, so a dragged tab will sit down and right of where it was picked
/// up by however far into it the press landed. Closing it means accumulating
/// the delta per commit onto the stored icon, which is a change to
/// `CompositorHandler::commit` and its own piece of work; an icon in roughly
/// the right place is not what #57 is about.
fn drag_icon(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    output_scale: Scale<f64>,
    scale: f64,
    shift: smithay::utils::Point<f64, smithay::utils::Logical>,
) -> Vec<Element> {
    let (Some(icon), Some(pointer)) = (state.dnd_icon(), state.seat.get_pointer()) else {
        return Vec::new();
    };
    // Every output draws it at its own offset and lets the renderer discard the
    // ones it misses, for the same reason `cursor` does: an icon crossing a
    // bezel has to be on both screens, and choosing one would cut it in half at
    // exactly the moment it is being carried across.
    let location = pointer.current_location() + shift;
    // Always zero today — `hotspot` says why — so this is the pointer's own
    // position, which is where the protocol puts an icon's top-left corner. It
    // is read rather than assumed because the day a drag icon does carry a
    // hotspot, the arithmetic that places it should already be the arithmetic
    // that places every other picture the pointer carries.
    let origin = origin_at(location, hotspot(&icon)).to_physical_precise_round(scale);
    render_elements_from_surface_tree::<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>(
        renderer,
        &icon,
        origin,
        output_scale,
        1.0,
        Kind::Unspecified,
    )
    .into_iter()
    .map(Element::Window2)
    .collect()
}

/// The pointer, however it is currently set.
///
/// A client that has set its own cursor gets that surface drawn; everything
/// else gets ours. `Hidden` draws nothing, which is a request clients make
/// deliberately — a video player going fullscreen, a game grabbing the pointer
/// — and not a failure.
fn cursor(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    output_scale: Scale<f64>,
    scale: f64,
    shift: smithay::utils::Point<f64, smithay::utils::Logical>,
    screen: smithay::utils::Size<i32, smithay::utils::Logical>,
) -> Vec<Element> {
    let Some(pointer) = state.seat.get_pointer() else {
        return Vec::new();
    };
    // The pointer is one thing in a global space and there are several screens
    // to draw it on. Each output draws it at its own offset, and the ones it is
    // not over draw it off their own edge, where the renderer discards it. No
    // test for "is the pointer on this monitor" is needed or wanted: a cursor
    // straddling the boundary has to appear on both, and picking one would clip
    // it to a half-cursor at the exact moment it crosses.
    //
    // A configured scene asks a narrower question, which keeps the straddle:
    // whether its picture reaches this output. It can animate, so drawing it
    // off every other monitor's edge would cost a full draw per monitor per
    // frame (`cursor::scene::tests::a_picture_is_drawn_only_on_the_outputs_it_touches`).
    let location = pointer.current_location() + shift;

    // `showing` rather than the field, which is now private. It is the one
    // reader, and it is where a cursor surface destroyed under a stationary
    // pointer turns back into the compositor's arrow instead of into a
    // surface tree that produces no elements. See `cursor::Pointer::showing`.
    match state.pointer.showing() {
        CursorImageStatus::Hidden => Vec::new(),
        CursorImageStatus::Surface(surface) => {
            let origin = origin_at(location, hotspot(&surface)).to_physical_precise_round(scale);
            let surface_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    &surface,
                    origin,
                    output_scale,
                    1.0,
                    Kind::Cursor,
                );
            surface_elements
                .into_iter()
                .map(|element| {
                    Element::Window(RescaleRenderElement::from_element(
                        element,
                        origin,
                        Scale::from(1.0),
                    ))
                })
                .collect()
        }
        // Already an `Element`, as a decoration is -- but *unlike* a
        // decoration, the pointer ends in a memory buffer on **both** paths,
        // and that is the whole point rather than an accident. A memory buffer
        // is the only thing smithay will put on the DRM cursor plane, so on the
        // GPU path Qt draws into a dmabuf and `cursor.rs` reads it straight
        // back out into one. See `cursor::Backing`, where that trade is argued.
        // A themed cursor is a memory buffer too, from a file rather than from
        // Qt, so it reaches the plane by the same route.
        //
        // The name is passed along now rather than discarded: it picks a cursor
        // out of the configured XCursor theme, and the QML pointer that used to
        // be the only answer here is what is drawn when there is no theme or
        // the theme has nothing under that name. See `cursor::Pointer::element`.
        //
        // A configured scene is drawn ahead of both, and it can animate, so
        // this asks for the next frame while it does, as `scripted` does for a
        // hosted scene: `cursor::scene::tests::an_animating_scene_asks_for_the_next_frame_only_while_it_animates`.
        CursorImageStatus::Named(icon) => {
            let drawn = state
                .pointer
                .element(renderer, icon, location, scale, screen);
            if drawn.animating {
                state.redraw = true;
            }
            drawn.element.into_iter().collect()
        }
    }
}

/// One window's elements, flat, at the origin and the size its pane has.
///
/// The offscreen pass draws these into a texture so a deformed window is
/// deformed as one thing. Built at the origin because the texture *is* the
/// window's own space; where it lands on screen is the warp's business.
///
/// **At the pane's size, from [`flat`], which for a tiled client that committed
/// more than its tile is the tile (#133).** `warp::mesh_part` spreads the whole
/// texture over the frame's rect, and that rect is the pane's; a capture at the
/// committed size pressed the whole buffer into the tile for the length of a
/// genie or a tilt, and told the frame the uncapped width as well, so its
/// titlebar was laid out at one width while warped and another once landed. The
/// client's surfaces are drawn at their own size, so what reaches past the tile
/// is simply off the texture's edge -- the cut `elements` makes with a crop,
/// made here by the framebuffer.
///
/// **A deformed window loses its bleed, and that is a known limit rather than
/// an oversight.** `offscreen::pane_job` sizes its texture from the window's
/// outer rect, so a layer placed at `(-bleed.left, -bleed.top)` falls outside
/// the framebuffer and is clipped by the renderer — the spikes are simply not
/// in the picture that gets bent. Fixing it means capturing at the decoration's
/// widest canvas *and* building `warp::mesh_part` over that larger rectangle,
/// since the mesh is what maps the texture back onto the window; both the
/// genie's anchor arithmetic and `crates/effects` are written against the
/// window's own rect today. It is `capture`'s change and the effects plan's,
/// not this one's. What it costs meanwhile is an effect that disappears while a
/// window is being deformed and comes back when it lands, which is visible but
/// is not wrong pixels.
///
/// **No popups.** A warped pane's popups are a capture of their own
/// (`offscreen::over_job`), drawn in front of the warp: inside this one they
/// sat under the titlebar and were cut at the window's edge.
/// `state::tests::a_warped_panes_capture_holds_no_popups`,
/// `tests::a_warped_panes_popups_are_in_front_of_it`.
///
/// **With its slots** (Ruling 17): every slot ready this pass, walked as the
/// flat path walks them ([`capture_pieces`]), each result placed over its
/// part in the capture, so a blurred window stays blurred while it is
/// warped. `prepare` builds this only once the chains have run
/// (`tests::a_warped_panes_capture_walks_its_client_slot`,
/// `tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`).
pub(crate) fn flat_window_elements(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    window: &Window,
    scale: f64,
    slots: &Slots,
) -> Vec<Element> {
    let pieces = state
        .panes
        .id_of(window)
        .map_or_else(|| PANE_ORDER.to_vec(), |pane| capture_pieces(slots, pane));
    flat_pane(
        state,
        renderer,
        window,
        scale,
        (0, 0).into(),
        slots,
        &pieces,
    )
}

/// [`flat_window_elements`] with the pane's corner at `at`, in the target's
/// physical pixels ([`pane_drawing_at`]): a whole pane's self capture,
/// padded by its chain's reach (`offscreen::part_job`), holding its inner
/// parts' results and never its own ([`inner_pieces`]).
/// `tests::a_pane_drawn_at_the_pad_has_its_client_inside_it`,
/// `tests::a_whole_panes_capture_walks_its_inner_slots_and_not_its_own`.
pub(crate) fn flat_window_elements_at(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    window: &Window,
    scale: f64,
    at: Point<i32, Physical>,
    slots: &Slots,
) -> Vec<Element> {
    let pieces = state
        .panes
        .id_of(window)
        .map_or_else(|| PANE_ORDER.to_vec(), |pane| inner_pieces(slots, pane));
    flat_pane(state, renderer, window, scale, at, slots, &pieces)
}

/// One pane drawn flat at its own size, its corner at `at`, walking
/// `pieces`: what both captures of a pane are drawn from.
/// `tests::a_warped_panes_capture_walks_its_client_slot`.
fn flat_pane(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    window: &Window,
    scale: f64,
    at: Point<i32, Physical>,
    slots: &Slots,
    pieces: &[Piece],
) -> Vec<Element> {
    let mut elements = Vec::new();
    let Some(Flat { outer, insets, .. }) = flat(state, window) else {
        return elements;
    };
    let output_scale = Scale::from(scale);
    // `None` only for a window smithay put in the space behind our back, which
    // `offscreen::pane_job` cannot produce -- it is holding the pane. Its layers
    // are skipped rather than the whole window, which is what the `if let`
    // around the old single `frame` call did: a window drawn without its chrome
    // is a window, and one skipped entirely is a hole in the picture.
    let pane = state.panes.id_of(window);

    // Fully opaque, and over the whole texture: this pass draws the window flat
    // at its real size and the warp applies the transform's opacity to the
    // whole texture afterwards, so applying it here as well would fade the
    // frame squared. The client within it.
    let (drawing, origin) = pane_drawing_at(outer, insets, at, scale);

    // Asked before the walk, which holds the state.
    let rounded = pane.and_then(|pane| clipped(state, pane));
    let radii = pane
        .and_then(|pane| declared_rounding(state, pane))
        .map(|effect| effect.radii());

    // The same walk a flat window goes through, so a tilted window carries its
    // layers and its slots in the order it would have had standing still. A
    // second sequence of calls here is how a deformed window would come to have
    // its `above` layer underneath its client.
    for &piece in pieces {
        match piece {
            Piece::Layers(depth) => {
                if let Some(pane) = pane {
                    chrome(
                        state,
                        renderer,
                        &mut elements,
                        Some(slots),
                        pane,
                        depth,
                        drawing,
                    );
                }
            }
            // A slot's result over its part at rest, unfaded: the client's
            // hole for the client, the pane's outer rectangle for the rest,
            // as the flat path's masks are at a zoom of one
            // (`tests::a_warped_panes_capture_walks_its_client_slot`).
            Piece::Slot(part, slot) => {
                let Some(pane) = pane else {
                    continue;
                };
                let mask = match part {
                    PaneSlot::Client => crate::effect::mask::client_mask(
                        present::logical(
                            (
                                drawing.rect.loc.x + f64::from(insets.left),
                                drawing.rect.loc.y + f64::from(insets.top),
                            ),
                            (
                                f64::from((outer.w - insets.horizontal()).max(1)),
                                f64::from((outer.h - insets.vertical()).max(1)),
                            ),
                        ),
                        radii,
                    ),
                    _ => crate::effect::mask::pane_mask(drawing.rect, radii),
                };
                elements.extend(slot_element(
                    state,
                    slots,
                    &crate::effect::plan::Owner::Pane(pane, part),
                    slot,
                    mask,
                    scale,
                    1.0,
                ));
            }
            // A style's rounding, drawn here as on the flat path, so a deformed
            // window keeps its corners: each surface through the clipped
            // programs, at real size, cut to what the capture holds of the client
            // (`tests::in_a_capture_a_client_is_clipped_to_what_the_capture_holds_of_it`).
            Piece::Client => {
                // **#232.** This path drew the raw surface at `origin`, nothing
                // taken off, so a client with client-side shadows (its window
                // geometry has a non-zero offset) had its picture shifted away
                // from its frame in every captured draw: warps, genies, close
                // fades (`pane_job`), and tilted `sol.present` presentations.
                // The same `window_surface_origin` the flat path (`elements`)
                // already used, so the two cannot drift apart again.
                // `state::tests::window_surface_origin_matches_a_csd_clients_shadow_offset`.
                let surface_origin = window_surface_origin(origin, window, scale);
                let surfaces = client_piece(renderer, window, surface_origin, output_scale);
                match &rounded {
                    Some((effect, programs)) => {
                        // `origin` for the room, `surface_origin` for the tree:
                        // the frame's content area does not move when the tree
                        // is shifted to re-place a CSD client's shadow, so the
                        // two anchors `capture_clip` takes must not be collapsed
                        // into one (`tests::a_shifted_tree_does_not_shrink_the_room_its_client_is_cut_to`).
                        let clip = capture_clip(
                            origin,
                            Size::<i32, Logical>::from((
                                outer.w - insets.horizontal(),
                                outer.h - insets.vertical(),
                            ))
                            .to_physical_precise_round(scale),
                            surface_origin,
                            window.geometry().to_physical_precise_round(scale),
                            crate::pass::physical_radii(*effect, scale),
                        );
                        elements.extend(surfaces.into_iter().map(|surface| {
                            Element::Clipped2(crate::clip::Clipped::new(
                                surface,
                                clip,
                                programs.clone(),
                            ))
                        }));
                    }
                    None => elements.extend(surfaces.into_iter().map(Element::Window2)),
                }
            }
        }
    }
    elements
}

/// A pane drawn flat at its own size with its corner at `at`, in a target's
/// physical pixels at `scale`: the drawing its layers are drawn with, and
/// where its client's corner goes, the insets inside it. At `(0, 0)`, a
/// warp's capture. `tests::a_pane_drawn_at_the_pad_has_its_client_inside_it`.
pub(crate) fn pane_drawing_at(
    outer: Size<i32, Logical>,
    insets: crate::decoration::Insets,
    at: Point<i32, Physical>,
    scale: f64,
) -> (crate::decoration::Drawing, Point<i32, Physical>) {
    let corner = at.to_f64().to_logical(scale);
    let drawing = crate::decoration::Drawing {
        rect: present::logical(
            (corner.x, corner.y),
            (f64::from(outer.w), f64::from(outer.h)),
        ),
        outer,
        alpha: 1.0,
        scale,
    };
    let client = at
        + Point::<i32, Logical>::from((insets.left, insets.top)).to_physical_precise_round(scale);
    (drawing, client)
}

/// The rectangle a warped window is captured at, and where its client sits in
/// it. See [`flat`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Flat {
    /// The texture's size, in logical pixels: the pane's outer size.
    pub(crate) outer: Size<i32, Logical>,
    /// The frame's share of it, which puts the client's corner.
    pub(crate) insets: crate::decoration::Insets,
    /// The client's hole in it: `outer` less the insets, at their corner.
    /// What a client's self capture holds
    /// (`state::tests::real_client::the_self_capture_is_padded_by_the_effects_reach`).
    pub(crate) client: Rectangle<i32, Logical>,
}

/// What `offscreen::pane_job` and [`flat_window_elements`] draw a warped window
/// at: **the pane's own outer rectangle**, which is the rectangle `elements`
/// builds the warp's mesh over, so the texture and the mesh are one size.
///
/// That is the tile for a tiled client that committed more than it (#133),
/// where the window's `outer_geometry` is the committed size:
/// `a_warped_window_is_captured_at_the_rect_its_warp_is_drawn_over` pins it.
/// It also counts the room a `Frame::Pending` pane reserves for a frame still
/// to be built, because `insets_of` does and `frame_insets` does not; that
/// half is read from the two functions and is not tested. A window with no
/// pane, which only smithay can put in the space, keeps `outer_geometry`.
pub(crate) fn flat(state: &Solium, window: &Window) -> Option<Flat> {
    let (outer, insets) = match state.panes.of(window) {
        Some(pane) => (state.pane_outer(pane).size, state.insets_of(pane.id())),
        None => (
            state.outer_geometry(window)?.size,
            state.frame_insets(window),
        ),
    };
    let client = Rectangle::new(
        (insets.left, insets.top).into(),
        (
            (outer.w - insets.horizontal()).max(1),
            (outer.h - insets.vertical()).max(1),
        )
            .into(),
    );
    Some(Flat {
        outer,
        insets,
        client,
    })
}

/// Where a pane's client is drawn inside a frame, and how its buffer is put
/// there. What [`place_client`] answers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placed {
    /// The drawn client rectangle: the frame's rect with the frame's share
    /// taken off, scaled as the frame canvas is scaled so the client sits in
    /// the hole the canvas leaves for it.
    pub(crate) client: Rectangle<f64, Logical>,
    /// Where the buffer's top-left corner is drawn. `client`'s corner, moved
    /// by a held picture's slack during a resize.
    pub(crate) origin: Point<f64, Logical>,
    /// How the buffer is scaled and cut.
    pub(crate) fit: Fit,
}

/// Where `pane`'s client is drawn inside `frame`, and how its buffer is fitted
/// to it.
///
/// `outer` is the pane's own outer size -- what its frame canvas is
/// rasterised at -- and `committed` the size its client committed. The one
/// answer every reader of a client's picture shares: `elements` draws the
/// surfaces through it, and `Solium::surface_under` inverts it, so a press
/// lands on the pixel the picture put there.
pub(crate) fn place_client(
    state: &Solium,
    pane: &Pane,
    frame: &present::Frame,
    outer: Size<i32, Logical>,
    committed: Size<i32, Logical>,
) -> Placed {
    // The frame's share, in drawn pixels. Scaled as the frame canvas is --
    // `decoration::spread` stretches the canvas from `outer` to the drawn rect
    // -- so the client fills the hole the canvas leaves for it.
    let insets = state.insets_of(pane.id());
    let across = ratio(frame.rect.size.w, outer.w);
    let down = ratio(frame.rect.size.h, outer.h);
    let client = present::logical(
        (
            frame.rect.loc.x + f64::from(insets.left) * across,
            frame.rect.loc.y + f64::from(insets.top) * down,
        ),
        (
            (frame.rect.size.w - f64::from(insets.horizontal()) * across).max(1.0),
            (frame.rect.size.h - f64::from(insets.vertical()) * down).max(1.0),
        ),
    );
    let fitting = fit(
        client,
        frame.zoom,
        committed,
        state.tile_of(pane).is_some(),
        state.resize_fill(pane.id()),
    );

    // **A picture that is not stretched stays against the edges the drag is
    // not moving.**
    //
    // `Fill::Hold` keeps the buffer at its own size where the pane has grown
    // past it, which leaves a strip the buffer does not cover. Anchoring at
    // the drawn rectangle's top-left puts that strip on the bottom and the
    // right, which is correct for a bottom or right drag -- those are exactly
    // the drags whose top-left corner is standing still -- and wrong for every
    // drag that pulls a left or top edge, where the picture would travel with
    // the pointer and the gap would open against the stationary edge. The
    // whole of the window's contents would slide while the user dragged one of
    // four corners.
    //
    // **Zero for a stretch, by the arithmetic rather than by a branch.** A
    // stretched buffer covers its rectangle exactly, so the slack below is
    // `client.size.w - real.size.w * (client.size.w / real.size.w)`, which is
    // nothing -- the default path and every window that is not being dragged
    // at all get the client's corner back, to the bit.
    let (pulls_left, pulls_top) = state.resize_pins(pane.id()).unwrap_or((false, false));
    let slack = |pulled: bool, drawn: f64, real: i32, factor: f64| {
        if pulled {
            (drawn - f64::from(real) * factor).max(0.0)
        } else {
            0.0
        }
    };
    let origin = client.loc
        + Point::<f64, Logical>::from((
            slack(pulls_left, client.size.w, committed.w, fitting.factor.x),
            slack(pulls_top, client.size.h, committed.h, fitting.factor.y),
        ));
    Placed {
        client,
        origin,
        fit: fitting,
    }
}

/// How a client's surfaces go into the rectangle they are drawn in: the
/// factor every surface is scaled by, about the client's corner, and the
/// rectangle they are cut to when there is one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fit {
    /// What the whole committed buffer is scaled by. Also what the popups are
    /// scaled by, which is why it is separate from the cut: they take this and
    /// never the other.
    pub(crate) factor: Scale<f64>,
    /// The drawn client rectangle, for a tiled client whose tile cuts some of
    /// what it committed. `None` for every other client -- every client that
    /// is not tiled, and every one its tile does not cut -- which is drawn
    /// through no crop at all, so a rounding in this rectangle cannot shave a
    /// pixel off a window that had nothing to cut. `fitted` rounds it to the
    /// output's pixels.
    pub(crate) crop: Option<Rectangle<f64, Logical>>,
    /// How much of the committed buffer, from its top-left corner, is in the
    /// picture: all of it, except along an axis the tile cuts. What a masked
    /// client is captured at, so its corners are the picture's corners.
    pub(crate) shown: Size<i32, Logical>,
}

impl Fit {
    /// The same fit with nothing cut: what a popup is drawn through. A menu
    /// scales with the window it belongs to and reaches past its tile.
    pub(crate) const fn uncut(self) -> Self {
        Self { crop: None, ..self }
    }
}

/// Less than this short of the committed size is not a cut. Half a logical
/// pixel, because the pictured size is float arithmetic -- a blended rect over
/// a blended zoom, less insets scaled by another quotient -- and a window that
/// fits can come back a rounding short of what it committed; a cut that small
/// would put a crop, and its physical rounding, on a window with nothing to
/// cut. A real cut is a whole pixel or more: a tile and a commit are both whole
/// logical pixels. The margin is argued here and not pinned by a test.
const CUT: f64 = 0.5;

/// A client's [`Fit`], from the rectangle it is drawn in and what it committed.
///
/// `client` is the drawn client rectangle and `zoom` the frame's
/// [`present::Frame::zoom`], so `client / zoom` is the client rectangle the
/// frame is a picture of, at the pane's own scale: the tile's share for a
/// settled tiled pane, a rectangle part of the way between two tiles on a
/// frame of a layout's glide, and the window's own size in a thumbnail.
/// `tiled` is whether a layout holds the pane in a tile, and `fill` is the
/// resize hold's, `None` when there is no hold.
///
/// **Per axis, the buffer is drawn at its own size where the pictured
/// rectangle is narrower than it and the pane is tiled** -- the tile cuts it --
/// and is scaled to fill the pictured rectangle everywhere else, which is the
/// arithmetic `render::elements` always had. Then the whole of it is scaled by
/// `zoom`, and the cut, when there is one, is `client` itself. So:
///
/// * A settled client that committed more than its tile is shown 1:1, cut to
///   the tile, and not squashed into it:
///   `a_tiled_client_is_cut_to_its_tile_and_not_squashed_into_it`.
/// * An open, a close or a thumbnail scales that cut picture as one:
///   `an_animated_tiled_client_is_scaled_with_its_cut_and_not_cut_by_it`.
/// * A glide that narrows a tiled window whose client has not answered yet is
///   the buffer 1:1 with the cut following the drawn rectangle, which is the
///   frame before it on the first frame and the settled window on the last:
///   `a_glide_that_narrows_a_tiled_window_cuts_it_and_does_not_zoom_it`. The
///   factor used to be `client / shown`, the old tile drawn as a zoom of the
///   new one: 2x on the first frame of a sweep that halved a window, which is
///   what `a_glide_that_narrows_a_tiled_window_is_drawn_1_to_1_on_its_first_frame`
///   measured against that arithmetic.
/// * A glide that grows a window past what its client committed stretches the
///   buffer into it, as every glide did before #133, and one that changes the
///   two axes by different amounts does each axis on its own:
///   `a_glide_between_tiles_of_different_shapes_is_fitted_per_axis`.
pub(crate) fn fit(
    client: Rectangle<f64, Logical>,
    zoom: (f64, f64),
    committed: Size<i32, Logical>,
    tiled: bool,
    fill: Option<crate::resizing::Fill>,
) -> Fit {
    // A frame drawn at no size pictures nothing in particular; the committed
    // size keeps the arithmetic finite, and the zoom draws it at nothing.
    let pictured = |drawn: f64, zoom: f64, committed: i32| {
        if zoom > f64::EPSILON {
            drawn / zoom
        } else {
            f64::from(committed)
        }
    };
    let pictured: Size<f64, Logical> = (
        pictured(client.size.w, zoom.0, committed.w),
        pictured(client.size.h, zoom.1, committed.h),
    )
        .into();
    let cuts = |pictured: f64, committed: i32| tiled && pictured < f64::from(committed) - CUT;
    let (cut_x, cut_y) = (cuts(pictured.w, committed.w), cuts(pictured.h, committed.h));
    let (across, down) = crate::resizing::factor(fill, pictured, committed);
    let factor = Scale::from((
        zoom.0 * if cut_x { 1.0 } else { across },
        zoom.1 * if cut_y { 1.0 } else { down },
    ));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a cut is narrower than the committed size, which is an i32"
    )]
    let kept = |cut: bool, pictured: f64, committed: i32| {
        if cut {
            (pictured.round() as i32).clamp(1, committed.max(1))
        } else {
            committed
        }
    };
    Fit {
        factor,
        crop: (cut_x || cut_y).then_some(client),
        shown: (
            kept(cut_x, pictured.w, committed.w),
            kept(cut_y, pictured.h, committed.h),
        )
            .into(),
    }
}

/// One client surface through its [`Fit`].
///
/// Generic over the element so that the two wrappers smithay puts round a
/// surface can be driven by a test with an element that needs no renderer --
/// what comes out is what `elements` hands the damage tracker.
#[derive(Debug)]
pub(crate) enum Fitted<E> {
    /// Nothing to cut: the client is not tiled, or fits its tile.
    Whole(RescaleRenderElement<E>),
    /// Cut to the client's tile.
    Cut(CropRenderElement<RescaleRenderElement<E>>),
}

impl Fitted<WaylandSurfaceRenderElement<GlesRenderer>> {
    /// Into the frame's element list.
    fn into_element(self) -> Element {
        match self {
            Self::Whole(whole) => Element::Window(whole),
            Self::Cut(cut) => Element::Tiled(cut),
        }
    }
}

impl Fitted<crate::clip::Clipped> {
    /// Into the frame's element list: a rounded client's surface.
    fn into_clipped(self) -> Element {
        match self {
            Self::Whole(each) => Element::ClippedWindow(each),
            Self::Cut(each) => Element::ClippedTiled(each),
        }
    }
}

impl Fitted<crate::remains::Surface> {
    /// Into the frame's element list. Not `into_element`, which the path
    /// `Fitted::into_element` above has to name without a type.
    fn into_remains(self) -> Element {
        match self {
            Self::Whole(whole) => Element::Remains(whole),
            Self::Cut(cut) => Element::RemainsTiled(cut),
        }
    }
}

/// What stands where the client of a window that has gone was, for one frame.
///
/// **Through [`place_client`] and [`fitted`], exactly as a live client's
/// surfaces go**, with the size the client last committed and the geometry it
/// was drawn at kept on the pane, so the picture lands on the pixels the client
/// occupied, is cut to the tile the window left (#133) and shrinks with its
/// frame. `frame` is the pane's drawn frame on this screen and `outer` its
/// outer size, the two things the live path hands `place_client`.
///
/// A picture with no pixels in it -- a client whose last buffer was never
/// imported -- is not drawn from at all: the client rectangle is filled with
/// `remains::FILL` instead, so what fades out is the window's shape and never
/// nothing. A picture with some draws each surface that has none as the same
/// fill, where that surface was (`remains::Picture::elements`).
///
/// **Two things the live path does are not done here**, both read and neither
/// tested, since no test has a GPU. A style's rounding (the `client.radius`
/// of `rounded` and `flush`) is not applied: `clip::Clipped` wraps a live
/// client's surface, which this is not, so under those styles the corners of
/// what fades are square from its first frame. And a matrix or a deform
/// (`Prepared::warp`: a tilt, a genie) is not either: what fades is flat at
/// `frame.rect`, so a tilted window snaps flat on its first frame and one
/// pulled into the dock by a genie pops back to full size before it fades. A
/// warp is keyed by the client's `Window`, which a window that has gone no
/// longer has, and [`prepare`] releases the pane's captures
/// (`keyed::Captures`) on the first frame it has none -- where the last
/// capture of it was, which it could have been drawn from. The default style,
/// `top`, has no rounding, and the tilt and the genie are what `init.lua`'s
/// dev bindings and `tweaks.lua`'s effects ask for.
pub(crate) fn remains_elements(
    state: &Solium,
    pane: crate::pane::PaneId,
    frame: &present::Frame,
    outer: Size<i32, Logical>,
    scale: f64,
) -> Vec<Element> {
    let Some(held) = state.panes.get(pane) else {
        return Vec::new();
    };
    let Some(left) = held.left() else {
        return Vec::new();
    };
    let output_scale = Scale::from(scale);
    let picture = match &left.remains {
        crate::pane::Remains::Picture(picture) if picture.drawable() => Some(picture),
        crate::pane::Remains::Picture(_) | crate::pane::Remains::Lost => None,
        // Drawn by `scene`, through the walk that draws a loading pane.
        crate::pane::Remains::Scene(_) => return Vec::new(),
    };
    if let Some(picture) = picture {
        let placed = place_client(state, held, frame, outer, picture.committed());
        let origin = placed.origin.to_physical_precise_round(scale);
        // Where the buffer's own corner goes: the window's rectangle starts
        // `inset` into it for a client that draws a shadow, which is the same
        // offset the live path takes off.
        let surfaces = origin - picture.inset().to_physical_precise_round(scale);
        return picture
            .elements(surfaces, scale, frame.opacity)
            .filter_map(|surface| {
                fitted(surface, origin, placed.fit, output_scale).map(Fitted::into_remains)
            })
            .collect();
    }
    let placed = place_client(state, held, frame, outer, left.geometry.size);
    let area = Rectangle::new(
        placed.client.loc.to_physical_precise_round(scale),
        placed.client.size.to_physical_precise_round(scale),
    );
    let fill = crate::remains::FILL;
    let alpha = fill[3] * frame.opacity;
    vec![Element::Solid(
        smithay::backend::renderer::element::solid::SolidColorRenderElement::new(
            left.fill.clone(),
            area,
            CommitCounter::default(),
            [fill[0] * alpha, fill[1] * alpha, fill[2] * alpha, alpha],
            Kind::Unspecified,
        ),
    )]
}

/// Put one surface through a fit. `None` when the cut leaves nothing of it,
/// which `CropRenderElement::from_element` answers for a surface lying wholly
/// outside the rectangle; the caller drops it.
pub(crate) fn fitted<E: smithay::backend::renderer::element::Element>(
    element: E,
    origin: Point<i32, Physical>,
    fit: Fit,
    output_scale: Scale<f64>,
) -> Option<Fitted<E>> {
    let scaled = RescaleRenderElement::from_element(element, origin, fit.factor);
    match fit.crop {
        None => Some(Fitted::Whole(scaled)),
        Some(crop) => {
            let crop = Rectangle::new(
                crop.loc.to_physical_precise_round(output_scale),
                crop.size.to_physical_precise_round(output_scale),
            );
            CropRenderElement::from_element(scaled, output_scale, crop).map(Fitted::Cut)
        }
    }
}

/// A toplevel's own surfaces, **without its popups**.
///
/// `Window::render_elements` is smithay's `AsRenderElements for Window`, and
/// for a Wayland toplevel it emits every popup from `popups_for_surface` ahead
/// of the toplevel's own tree (`desktop/space/wayland/window.rs:106-121` in
/// smithay 0.7). Every path that draws a client here draws its popups itself,
/// so going through it drew each popup twice: once where the path put it, and
/// once as part of the toplevel -- cut to the tile with the toplevel, and
/// under a masked client's corners. An opaque popup hides its double; a
/// translucent pixel -- a menu's shadow, a rounded corner, any popup in a
/// fading window -- blends twice inside the tile and once past it, which is a
/// seam at the tile's edge. So a Wayland toplevel is drawn from its own
/// surface tree, which holds its subsurfaces and nothing else. An X11 window
/// has no popups here -- its menus are windows of their own -- and keeps
/// smithay's call. `a_toplevel_is_drawn_without_its_popups` pins it.
///
/// Generic over the renderer so that test can run it on smithay's
/// `DummyRenderer`: it needs no GPU, and what it returns is the element list
/// every caller builds on.
pub(crate) fn toplevel_elements<R>(
    renderer: &mut R,
    window: &Window,
    location: Point<i32, Physical>,
    scale: Scale<f64>,
    alpha: f32,
) -> Vec<WaylandSurfaceRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    match window.toplevel() {
        Some(toplevel) => render_elements_from_surface_tree(
            renderer,
            toplevel.wl_surface(),
            location,
            scale,
            alpha,
            Kind::Unspecified,
        ),
        None => window.render_elements(renderer, location, scale, alpha),
    }
}

/// A toplevel's popups, drawn from the client's corner at `origin`, and the
/// rectangle they cover relative to that corner, which may reach past the
/// window. The one walk of `popups_for_surface`: the flat path draws these in
/// front of the whole sandwich, a warped pane in front of its warp (Ruling 15).
/// `state::tests::a_warped_panes_capture_holds_no_popups`,
/// `state::tests::a_popup_past_the_window_is_captured_whole`.
pub(crate) fn popup_elements<R>(
    renderer: &mut R,
    window: &Window,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
    alpha: f32,
) -> (
    Vec<WaylandSurfaceRenderElement<R>>,
    Option<Rectangle<i32, Logical>>,
)
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let mut elements = Vec::new();
    let Some(surface) = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone())
    else {
        return (elements, None);
    };
    for (popup, offset) in PopupManager::popups_for_surface(&surface) {
        let at = origin + (offset - popup.geometry().loc).to_physical_precise_round(scale);
        elements.extend(render_elements_from_surface_tree(
            renderer,
            popup.wl_surface(),
            at,
            scale,
            alpha,
            Kind::Unspecified,
        ));
    }
    (elements, popups_covered(window))
}

/// The rectangle a toplevel's popups cover, relative to its client's corner,
/// which may reach past the window; `None` with none open. The tree where it
/// is drawn, not each popup's window geometry: a popup that sets none has a
/// zero-sized one, and what a capture of it must hold is everything drawn.
/// `state::tests::a_warped_panes_capture_holds_no_popups`,
/// `state::tests::a_popup_past_the_window_is_captured_whole`.
pub(crate) fn popups_covered(window: &Window) -> Option<Rectangle<i32, Logical>> {
    let surface = window.toplevel()?.wl_surface().clone();
    PopupManager::popups_for_surface(&surface)
        .map(|(popup, offset)| {
            smithay::desktop::utils::bbox_from_surface_tree(
                popup.wl_surface(),
                offset - popup.geometry().loc,
            )
        })
        .reduce(|held, rect| held.merge(rect))
}

/// Where a client's surface itself is drawn, given the corner `origin`
/// reserves for it: a surface's top-left is not the window's. A client that
/// draws its own decorations puts its drop shadow *outside* the window
/// geometry and tells us so through `xdg_surface.set_window_geometry`;
/// drawing the surface at `origin` unchanged therefore lands the shadow
/// where the window should be and pushes the window itself down and right
/// by the shadow's width (#232). A window that sets no geometry has a
/// zero offset, so `origin` comes back unchanged.
///
/// Pure, and taking `window` rather than running the whole draw, so the
/// formula can be pinned without a `Solium` or a `GlesRenderer`, neither of
/// which a unit test in this crate can build (the same reasoning as
/// [`origin_at`]'s doc). Shared by the flat path (`elements`) and the
/// captured one (`flat_window_elements`, by way of `client_piece`), so the
/// two cannot drift back apart the way #232 found them.
/// `state::tests::window_surface_origin_matches_a_csd_clients_shadow_offset`.
pub(crate) fn window_surface_origin(
    origin: Point<i32, Physical>,
    window: &Window,
    scale: f64,
) -> Point<i32, Physical> {
    origin - window.geometry().loc.to_physical_precise_round(scale)
}

/// What a pane's capture draws of its client: its own surface tree, no
/// popups. `state::tests::a_warped_panes_capture_holds_no_popups`.
pub(crate) fn client_piece<R>(
    renderer: &mut R,
    window: &Window,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
) -> Vec<WaylandSurfaceRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    toplevel_elements(renderer, window, origin, scale, 1.0)
}

/// Drawn size over real size, guarding the degenerate case.
///
/// A zero-sized window is not drawable, but it is reachable: a client can
/// commit before it has been configured. Scaling by zero would collapse the
/// element and scaling by infinity would take the renderer with it.
///
/// Shared with `decoration::spread`, which needs exactly this number for
/// exactly this reason: a layer's bleed scales with the window its layer
/// belongs to, and two definitions of "how much has this window been scaled by"
/// is how a frame and its bleed come to be scaled differently.
pub(crate) fn ratio(drawn: f64, real: i32) -> f64 {
    if real <= 0 || drawn <= 0.0 {
        1.0
    } else {
        drawn / f64::from(real)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Drawn, Fit, Fitted, Painted, by_depth, fit, fitted, origin_at, ratio};
    use crate::qml::qt_test::on_the_qt_thread;

    /// **The shape key moves with the part's box and its radii**, and only
    /// with them: what a `depends = "shape"` state is rebuilt on.
    #[test]
    fn the_shape_key_moves_with_the_box_and_the_radii() {
        let content = [0.1, 0.1, 0.8, 0.8];
        assert_eq!(
            super::shape_hash(content, [6.0; 4]),
            super::shape_hash(content, [6.0; 4])
        );
        assert_ne!(
            super::shape_hash(content, [6.0; 4]),
            super::shape_hash(content, [8.0; 4])
        );
        assert_ne!(
            super::shape_hash(content, [6.0; 4]),
            super::shape_hash([0.1, 0.1, 0.8, 0.79], [6.0; 4])
        );
    }

    /// A machine whose every draw fails: it counts the phase's `begin`, its
    /// runs and its `end`, and answers each run with `run::preflight`'s
    /// outcome, else `Outcome::Failed`; it has no blank, so a T0 chain runs
    /// on it with no texture (Tasks 22 and 24 drive it).
    #[derive(Debug, Default)]
    pub(crate) struct NoGpu {
        pub(crate) begins: u32,
        pub(crate) runs: u32,
        pub(crate) ends: u32,
    }

    impl super::Runner for NoGpu {
        fn begin(&mut self) {
            self.begins += 1;
        }
        fn run<'p>(
            &mut self,
            _pool: &mut crate::pool::Pool,
            programs: &dyn Fn(u64) -> crate::effect::run::Lookup<'p>,
            formats: Option<crate::pool::Formats>,
            plan: &solium_effects::stage::Plan,
            _held: &mut crate::effect::run::Held,
            _inputs: &crate::effect::run::Inputs<'_>,
            _keys: &crate::effect::run::Keys,
        ) -> crate::effect::run::Outcome {
            self.runs += 1;
            crate::effect::run::preflight(plan, programs, formats)
                .unwrap_or(crate::effect::run::Outcome::Failed)
        }
        fn end(&mut self) {
            self.ends += 1;
        }
        fn blank(
            &mut self,
            _pool: &mut crate::pool::Pool,
        ) -> Option<smithay::backend::renderer::gles::GlesTexture> {
            None
        }
    }

    /// **Only a whole pane's self chain runs in the later phase**: its
    /// capture walks what the inner slots made (Task 25), so it is `Whole`;
    /// every other slot, and a whole pane's chain that reads nothing of the
    /// frame, is `Inner`.
    #[test]
    fn only_a_whole_panes_self_chain_runs_in_the_later_phase() {
        use super::Nest;
        use crate::effect::plan::{Owner, PaneSlot};
        use crate::effect::rules::Tier;
        let pane = crate::pane::PaneId::from_raw(1);
        assert_eq!(
            Nest::of(&Owner::Pane(pane, PaneSlot::Pane), Tier::Own),
            Nest::Whole
        );
        for part in [
            PaneSlot::Client,
            PaneSlot::Popups,
            PaneSlot::Layer(0),
            PaneSlot::Titlebar,
        ] {
            assert_eq!(Nest::of(&Owner::Pane(pane, part), Tier::Own), Nest::Inner);
        }
        assert_eq!(
            Nest::of(&Owner::Pane(pane, PaneSlot::Pane), Tier::Generated),
            Nest::Inner
        );
        assert_eq!(
            Nest::of(
                &Owner::Surface(crate::scripted::SurfaceId::from_raw(1), "DP-1".to_owned()),
                Tier::Own
            ),
            Nest::Inner
        );
    }

    /// **A self slot with no input this pass runs nothing**: its capture was
    /// neither drawn nor kept (its owner gone, or no target for it), so its
    /// slot is not ready and the walk draws the part as with no rule; and a
    /// phase that runs nothing opens no GPU region (Ruling 10).
    #[test]
    fn a_self_slot_with_no_input_runs_nothing_and_opens_no_region() {
        use crate::effect::plan::{Chains, Owner, PaneSlot, PartBox, Slots};
        use crate::effect::rules::{Origin, RuleKey, Slot};
        let place = crate::effect::host::tests::scratch("render-no-input");
        crate::effect::host::tests::folder(
            &place,
            "tint",
            "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        let mut host = crate::effect::host::Host::new(crate::effect::host::Library::with(
            Some(place.clone()),
            place.join("none"),
        ));
        host.want("rules", ["tint".to_owned()]);
        let lua = mlua::Lua::new();
        let value: mlua::Value = lua
            .load(r#"{ { match = "*", part = "client", slot = "behind", effect = "tint" } }"#)
            .eval()
            .expect("the test's Lua");
        let tree = crate::effect::tree::Tree::from_lua(&value)
            .expect("readable")
            .expect("a value");
        let rule = crate::effect::rules::parse(&tree)
            .expect("parses")
            .remove(0);
        let key = RuleKey {
            origin: Origin::User,
            index: 0,
            generation: 1,
        };
        let mut chains = Chains::default();
        chains.insert(key, Chains::bind(&mut host, &rule).expect("binds"));
        let owner = Owner::Pane(crate::pane::PaneId::from_raw(1), PaneSlot::Client);
        let mut slots = Slots::default();
        slots.want(owner.clone(), Slot::Behind, key);
        slots.set_box(
            owner.clone(),
            Slot::Behind,
            PartBox::around((100, 80).into(), 0, [0.0; 4]),
        );
        let mut store = crate::effect::store::Store::default();
        let mut cx = super::RunCx {
            store: &mut store,
            effects: &host,
            chains: &mut chains,
            masked: None,
            now: 0.0,
            pass: 1,
        };
        let (mut runner, mut pool) = (NoGpu::default(), crate::pool::Pool::new(0));
        for nest in [super::Nest::Inner, super::Nest::Whole] {
            super::run_slots(
                &mut cx,
                &mut pool,
                &mut runner,
                &mut slots,
                &crate::effect::store::Drawn::default(),
                nest,
            );
        }
        assert_eq!((runner.begins, runner.runs, runner.ends), (0, 0, 0));
        assert!(!slots.is_ready(&owner, Slot::Behind));
        let _ = std::fs::remove_dir_all(place);
    }

    /// A T0 slot never counts its input as redrawn, so a client committing
    /// under a generated effect does not re-run it.
    #[test]
    fn a_t0_slot_never_counts_as_redrawn() {
        use crate::effect::rules::Tier;
        assert!(!super::input_redrawn(Tier::Generated, true));
        assert!(super::input_redrawn(Tier::Own, true));
        assert!(!super::input_redrawn(Tier::Own, false));
    }

    /// **Twenty runs in a phase open one GPU region** (Ruling 10): \[fx0\]
    /// Task 3's ring times sixteen regions a pass and counts the rest as
    /// `refused`, so one region per run would read low with a dozen slots.
    /// The twenty are generated (T0) chains, which read nothing of the frame
    /// and so run with no self input drawn.
    #[test]
    fn twenty_runs_in_a_phase_open_one_region() {
        use crate::effect::plan::{Chains, Owner, PaneSlot, PartBox, Slots};
        use crate::effect::rules::{Origin, RuleKey, Slot};
        let place = crate::effect::host::tests::scratch("twenty-runs");
        crate::effect::host::tests::folder(
            &place,
            "glow",
            "return { api = 1, inputs = { 'shape' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return vec4(sol_shape(uv)); }\n",
            )],
        );
        let mut host = crate::effect::host::Host::new(crate::effect::host::Library::with(
            Some(place.clone()),
            place.join("none"),
        ));
        host.want("rules", ["glow".to_owned()]);
        let lua = mlua::Lua::new();
        let value: mlua::Value = lua
            .load(r#"{ { match = "*", part = "client", slot = "behind", effect = "glow" } }"#)
            .eval()
            .expect("the test's Lua");
        let tree = crate::effect::tree::Tree::from_lua(&value)
            .expect("readable")
            .expect("a value");
        let rule = crate::effect::rules::parse(&tree)
            .expect("parses")
            .remove(0);
        let (mut chains, mut slots) = (Chains::default(), Slots::default());
        let part = PartBox::around((120, 80).into(), 12, [0.0; 4]);
        for index in 1..=20_u32 {
            let key = RuleKey {
                origin: Origin::User,
                index,
                generation: 1,
            };
            chains.insert(key, Chains::bind(&mut host, &rule).expect("binds"));
            let owner = Owner::Pane(
                crate::pane::PaneId::from_raw(u64::from(index)),
                PaneSlot::Client,
            );
            slots.want(owner.clone(), Slot::Behind, key);
            slots.set_box(owner, Slot::Behind, part);
        }
        let mut store = crate::effect::store::Store::default();
        let mut cx = super::RunCx {
            store: &mut store,
            effects: &host,
            chains: &mut chains,
            masked: None,
            now: 0.0,
            pass: 1,
        };
        let mut runner = NoGpu::default();
        super::run_slots(
            &mut cx,
            &mut crate::pool::Pool::new(0),
            &mut runner,
            &mut slots,
            &crate::effect::store::Drawn::default(),
            super::Nest::Inner,
        );
        assert_eq!(
            (runner.begins, runner.runs, runner.ends),
            (1, 20, 1),
            "one region a phase, not one a run"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A window whose warp has no program is not captured**, and goes the
    /// flat way, rounded corners included, rather than being drawn as nothing.
    #[test]
    fn a_window_whose_warp_has_no_program_is_drawn_flat() {
        use super::{Route, route};
        assert_eq!(route(true, true), Route::Warp);
        assert_eq!(route(true, false), Route::Flat, "no program, no warp");
        assert_eq!(route(false, true), Route::Flat, "nothing to warp");
    }

    /// **A bare pane gets no style rules**: fullscreen, CSD and `pane =
    /// "none"` panes have no `Decoration`, so a style's rules never reach them
    /// (Ruling 15).
    #[test]
    fn a_bare_pane_gets_no_style_rules() {
        let frame = crate::pane::Frame::None;
        assert!(super::style_rules(&frame).0.is_empty());
        assert_eq!(super::style_rules(&crate::pane::Frame::Pending).1, 0);
    }

    /// **The guard (spec §8.4): a pane neither warped nor styled wants no
    /// capture**, so it builds no job, and a pass of such panes binds no
    /// carrier: `offscreen::draw` returns at once on an empty list. A warp
    /// wants its pane's capture, and nothing else does: rounding is drawn
    /// inline.
    #[test]
    fn a_pane_neither_warped_nor_styled_wants_no_capture() {
        assert_eq!(super::wanted_capture(super::Route::Flat), None);
        assert_eq!(
            super::wanted_capture(super::Route::Warp),
            Some(crate::keyed::Kind::Pane)
        );
    }

    /// The guard: a style with no radius draws its client as before, with no
    /// clip and no program: the unstyled path is untouched (spec §8.4).
    #[test]
    fn a_style_with_no_radius_wraps_nothing() {
        assert!(super::rounding(&[]).is_none());
        let none =
            solium_effects::fragment::Effect::rounded(solium_effects::fragment::Corners::all(0.0));
        assert!(
            super::rounding(&[none]).is_none(),
            "a zero radius is no effect"
        );
    }

    /// And a style with a radius is drawn inline, never captured.
    #[test]
    fn a_style_with_a_radius_is_drawn_inline() {
        let rounded =
            solium_effects::fragment::Effect::rounded(solium_effects::fragment::Corners::all(12.0));
        assert_eq!(super::rounding(&[rounded]), Some(rounded));
        assert_eq!(rounded.inputs(), solium_effects::fragment::Inputs::Inline);
    }

    /// The rounding is the first one *with a radius*, not the first in the
    /// list: a style that declared a zero radius and then a real one would
    /// otherwise round nothing, and a one-element list cannot tell "skipped
    /// it" from "stopped at it".
    #[test]
    fn a_none_effect_does_not_hide_the_rounding_behind_it() {
        use solium_effects::fragment::{Corners, Effect};
        let rounded = Effect::rounded(Corners::all(8.0));
        assert_eq!(
            super::rounding(&[Effect::rounded(Corners::all(0.0)), rounded]),
            Some(rounded)
        );
    }

    /// And of two, the first wins.
    #[test]
    fn of_two_roundings_the_first_wins() {
        use solium_effects::fragment::{Corners, Effect};
        let first = Effect::rounded(Corners::all(4.0));
        let second = Effect::rounded(Corners::all(12.0));
        assert_eq!(super::rounding(&[first, second]), Some(first));
    }

    /// **A zoomed client is clipped to the whole of itself.** The clip is
    /// measured in the surfaces' own pixels, before the rescale
    /// (`clip::input_to_geo`), so the client's rectangle as drawn is taken
    /// back through it: a 300x200 client drawn at half size, 150x100, is
    /// clipped to 300x200. Clipped to the 150x100 it is drawn at, the
    /// window in an overview or opening would lose all but its top-left
    /// quarter. At 1:1 it is the drawn rectangle itself, a held picture's
    /// slack included.
    #[test]
    fn a_zoomed_clients_clip_is_the_whole_client() {
        use smithay::utils::{Physical, Rectangle, Scale};
        use solium_effects::fragment::Corners;
        let radii = Corners::all(12.0);
        let half = super::drawn_clip(
            Rectangle::<i32, Physical>::new((110, 220).into(), (150, 100).into()),
            (110, 220).into(),
            Scale::from(0.5),
            radii,
        );
        assert_eq!(
            half.rect,
            Rectangle::new((110.0, 220.0).into(), (300.0, 200.0).into())
        );
        assert_eq!(
            (half.origin, half.factor, half.radii),
            ((110, 220).into(), Scale::from(0.5), radii)
        );
        let held = super::drawn_clip(
            Rectangle::<i32, Physical>::new((100, 200).into(), (150, 100).into()),
            (110, 220).into(),
            Scale::from(1.0),
            radii,
        );
        assert_eq!(
            held.rect,
            Rectangle::new((100.0, 200.0).into(), (150.0, 100.0).into())
        );
    }

    /// **In a warp's capture a client is clipped to what the capture holds of
    /// it**: its geometry where the capture draws its tree, cut to the room
    /// the frame leaves it, so a tiled client that committed more than its
    /// tile is rounded at the tile's corners, where the capture's edge cuts it.
    #[test]
    fn in_a_capture_a_client_is_clipped_to_what_the_capture_holds_of_it() {
        use smithay::utils::{Physical, Rectangle};
        use solium_effects::fragment::Corners;
        let radii = Corners::all(12.0);
        let wide = super::capture_clip(
            (10, 30).into(),
            (300, 200).into(),
            (10, 30).into(),
            Rectangle::<i32, Physical>::from_size((400, 260).into()),
            radii,
        );
        assert_eq!(
            wide.rect,
            Rectangle::new((10.0, 30.0).into(), (300.0, 200.0).into())
        );
        let small = super::capture_clip(
            (10, 30).into(),
            (300, 200).into(),
            (10, 30).into(),
            Rectangle::<i32, Physical>::from_size((250, 150).into()),
            radii,
        );
        assert_eq!(
            small.rect,
            Rectangle::new((10.0, 30.0).into(), (250.0, 150.0).into())
        );
        assert_eq!(
            (small.origin, small.factor),
            ((10, 30).into(), smithay::utils::Scale::from(1.0)),
            "drawn at real size"
        );
    }

    /// **A shifted tree does not shrink the room its client is cut to.**
    /// #232 moves a CSD client's tree off the frame's own origin so its
    /// shadow lands where its geometry says, but the frame's content area
    /// does not move with it: a frame at (0, 0), a shadow offset of (10, 15)
    /// (so the tree is drawn at (-10, -15)) and 44x49 of real content, with
    /// no tile overflow (`hole` exactly the content's size), must clip to the
    /// whole 44x49, not a (10, 15)-smaller rectangle reproducing the shadow
    /// offset -- the regression the two separate origins above guard against.
    #[test]
    fn a_shifted_tree_does_not_shrink_the_room_its_client_is_cut_to() {
        use smithay::utils::{Physical, Rectangle};
        use solium_effects::fragment::Corners;
        let radii = Corners::all(12.0);
        let shadow = (10, 15);
        let content = (44, 49);
        let clip = super::capture_clip(
            (0, 0).into(),
            content.into(),
            (-shadow.0, -shadow.1).into(),
            Rectangle::<i32, Physical>::new(shadow.into(), content.into()),
            radii,
        );
        assert_eq!(
            clip.rect,
            Rectangle::new((0.0, 0.0).into(), (44.0, 49.0).into())
        );
    }

    /// **A warped pane's popups are in front of it**, as on the flat path.
    #[test]
    fn a_warped_panes_popups_are_in_front_of_it() {
        assert_eq!(
            super::WARP_ORDER,
            [super::WarpPiece::Over, super::WarpPiece::Pane]
        );
    }

    /// The popups' part of the pane: their rectangle, moved by the client's
    /// corner in the pane, over the pane's size; past the pane, past 1.
    #[test]
    fn the_popups_part_is_their_rectangle_over_the_pane() {
        let outer = smithay::utils::Size::from((400, 300));
        let corner = smithay::utils::Point::from((0, 30));
        let covered = smithay::utils::Rectangle::new((300, 240).into(), (200, 90).into());
        let part = super::over_part(outer, corner, covered);
        assert_eq!(
            part,
            crate::warp::UnitRect {
                u0: 0.75,
                v0: 0.9,
                u1: 1.25,
                v1: 1.2
            }
        );
    }

    /// The Rust genie's grid at progress 1 from `frame` toward `to`, global:
    /// what these tests build their grid with since a geometry is a folder
    /// (its Lua is held to this grid by
    /// `warp::tests::a_lua_grid_lands_where_the_rust_genie_put_it`).
    fn genie_to(
        frame: &crate::present::Frame,
        to: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
    ) -> crate::warp::Grid {
        crate::warp::oracle_grid(
            frame.rect,
            to,
            solium_effects::Deform::Genie {
                progress: 1.0,
                spread: 1.4,
                axis: solium_effects::Axis::Down,
            },
            crate::warp::UnitRect::WHOLE,
        )
    }

    /// **A genie on the second monitor lands on its target**: at progress 1 the
    /// whole window is inside the target, moved onto the screen with it.
    #[test]
    fn a_genie_on_the_second_monitor_lands_on_its_target() {
        let screen = smithay::utils::Rectangle::new((1920, 0).into(), (1920, 1080).into());
        let frame = crate::present::Frame::real(smithay::utils::Rectangle::new(
            (2100, 100).into(),
            (800, 600).into(),
        ));
        let grid = genie_to(
            &frame,
            crate::present::logical((2800.0, 1000.0), (120.0, 24.0)),
        );
        let mesh = super::warp_mesh_on(
            screen,
            &frame,
            Some(&grid),
            crate::warp::UnitRect::WHOLE,
            1.25,
        )
        .expect("a mesh");
        let (left, top) = ((2800.0 - 1920.0) * 1.25, 1000.0 * 1.25);
        for corner in mesh.vertices() {
            let (x, y) = (f64::from(corner.x), f64::from(corner.y));
            assert!(
                x >= left - 1e-3
                    && x <= left + 150.0 + 1e-3
                    && y >= top - 1e-3
                    && y <= top + 30.0 + 1e-3,
                "({x}, {y}) is outside the target on the screen"
            );
        }
    }

    /// The guard: on the first monitor, nothing moves.
    #[test]
    fn a_genie_on_the_first_monitor_is_unchanged() {
        let screen = smithay::utils::Rectangle::new((0, 0).into(), (1920, 1080).into());
        let frame = crate::present::Frame::real(smithay::utils::Rectangle::new(
            (180, 100).into(),
            (800, 600).into(),
        ));
        let grid = genie_to(
            &frame,
            crate::present::logical((880.0, 1000.0), (120.0, 24.0)),
        );
        let on = super::warp_mesh_on(
            screen,
            &frame,
            Some(&grid),
            crate::warp::UnitRect::WHOLE,
            1.25,
        )
        .expect("a mesh");
        let direct = crate::warp::mesh_grid(
            &grid,
            (0.0, 0.0).into(),
            frame.rect,
            frame.matrix,
            frame.pivot,
            1.25,
        )
        .expect("a mesh");
        let pairs = on.vertices().iter().zip(direct.vertices());
        assert!(
            pairs.clone().count() > 0 && pairs.into_iter().all(|(a, b)| a.x == b.x && a.y == b.y)
        );
    }

    /// **A refused present follows its `failed`** (`effects.present.failed`,
    /// or the deform's own): `"flat"` draws the window undeformed, `"hide"`
    /// draws nothing of it and wants no capture; a mesh that was not refused
    /// is warped.
    #[test]
    fn a_refused_present_follows_its_failed_policy() {
        use crate::effect::settings::PresentFailed;
        let geometry = crate::present::Geometry::for_test;
        assert_eq!(
            super::present_route(geometry(PresentFailed::Flat), true),
            super::Route::Flat
        );
        assert_eq!(
            super::present_route(geometry(PresentFailed::Hide), true),
            super::Route::Hidden
        );
        assert_eq!(
            super::present_route(geometry(PresentFailed::Hide), false),
            super::Route::Warp
        );
        assert_eq!(super::wanted_capture(super::Route::Hidden), None);
    }

    /// **#133: what a tiled client's surfaces become on their way to the
    /// damage tracker**, with smithay's own wrappers and a stand-in surface.
    ///
    /// `elements` cannot be driven here -- it needs a `GlesRenderer`, which
    /// needs a GPU the build container does not have; see
    /// `the_drag_icon_is_emitted_below_the_lock_screens_early_return`. What it
    /// hands on for a client surface is `fitted(surface, origin, fit(..), ..)`,
    /// and `fitted` is generic over the surface, so these tests hand it one
    /// that needs no renderer and read back the `geometry` and `src` smithay's
    /// `RescaleRenderElement` and `CropRenderElement` report for it: the
    /// numbers the damage tracker draws with.
    mod fitting {
        use super::{Fit, Fitted, fit, fitted};
        use smithay::backend::renderer::element::{Element, Id};
        use smithay::backend::renderer::utils::CommitCounter;
        use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size};

        /// A surface with an untransformed buffer of `size` physical pixels,
        /// drawn at `at`: for such a buffer, the geometry and the source
        /// rectangle are what the two wrappers read from the element inside
        /// them (`element/utils/elements.rs` in smithay 0.7).
        struct Surface {
            id: Id,
            at: Point<i32, Physical>,
            size: Size<i32, Physical>,
        }

        impl Surface {
            fn new(at: (i32, i32), size: (i32, i32)) -> Self {
                Self {
                    id: Id::new(),
                    at: at.into(),
                    size: size.into(),
                }
            }
        }

        impl Element for Surface {
            fn id(&self) -> &Id {
                &self.id
            }

            fn current_commit(&self) -> CommitCounter {
                CommitCounter::default()
            }

            fn src(&self) -> Rectangle<f64, Buffer> {
                Rectangle::from_size((f64::from(self.size.w), f64::from(self.size.h)).into())
            }

            fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
                Rectangle::new(self.at, self.size)
            }
        }

        /// What one surface is drawn as: whether it was cut, where on the
        /// output, and which part of its buffer.
        #[derive(Debug, PartialEq)]
        struct Shown {
            cut: bool,
            geometry: Rectangle<i32, Physical>,
            src: Rectangle<f64, Buffer>,
        }

        const fn cut(geometry: Rectangle<i32, Physical>, src: Rectangle<f64, Buffer>) -> Shown {
            Shown {
                cut: true,
                geometry,
                src,
            }
        }

        const fn whole(geometry: Rectangle<i32, Physical>, src: Rectangle<f64, Buffer>) -> Shown {
            Shown {
                cut: false,
                geometry,
                src,
            }
        }

        /// One surface through `fitted`, read back. `None` for a surface the
        /// fit dropped.
        fn drawn(surface: Surface, origin: (i32, i32), fitting: Fit, scale: f64) -> Option<Shown> {
            let scale = Scale::from(scale);
            Some(match fitted(surface, origin.into(), fitting, scale)? {
                Fitted::Whole(scaled) => whole(scaled.geometry(scale), scaled.src()),
                Fitted::Cut(scaled) => cut(scaled.geometry(scale), scaled.src()),
            })
        }

        fn logical(x: f64, y: f64, w: f64, h: f64) -> Rectangle<f64, Logical> {
            Rectangle::new((x, y).into(), (w, h).into())
        }

        fn physical(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
            Rectangle::new((x, y).into(), (w, h).into())
        }

        fn buffer(w: f64, h: f64) -> Rectangle<f64, Buffer> {
            Rectangle::from_size((w, h).into())
        }

        /// The zoom of a frame drawn at the rectangle it has: at rest, and on
        /// every frame of a layout's glide. See `present::Frame::zoom`.
        const AT_REST: (f64, f64) = (1.0, 1.0);

        /// **A client that committed more than its tile is cut to the tile, at
        /// its own scale.**
        ///
        /// A kitty that stayed 1000 wide in a tile with room for 500. On stage
        /// `render::elements` scaled the surfaces by the drawn rect over the
        /// committed size and cut nothing, which after `pane_geometry` caps the
        /// pane is the squash: all 1000 pixels of buffer pressed into 500 of
        /// screen. Before the cap it was the spill -- the same buffer at 1:1,
        /// 500 pixels of it over the neighbour. The answer is neither: 1:1,
        /// and only the tile's 500 of it shown.
        ///
        /// Asserted at two output scales, because the cut is physical pixels
        /// and the tile is logical ones.
        #[test]
        fn a_tiled_client_is_cut_to_its_tile_and_not_squashed_into_it() {
            let committed = Size::from((1000, 600));
            let client = logical(100.0, 50.0, 500.0, 600.0);

            let settled = fit(client, AT_REST, committed, true, None);
            assert_eq!(
                settled.factor,
                Scale::from((1.0, 1.0)),
                "the buffer is drawn at its own size; anything else is a squash"
            );
            assert_eq!(
                settled.shown,
                Size::from((500, 600)),
                "a masked client is captured at the tile's share"
            );
            assert_eq!(
                drawn(
                    Surface::new((100, 50), (1000, 600)),
                    (100, 50),
                    settled,
                    1.0
                ),
                Some(cut(physical(100, 50, 500, 600), buffer(500.0, 600.0))),
                "the surface is cut to the tile: 500 pixels of screen, showing the \
                 first 500 pixels of the buffer"
            );
            assert_eq!(
                drawn(
                    Surface::new((200, 100), (2000, 1200)),
                    (200, 100),
                    settled,
                    2.0
                ),
                Some(cut(physical(200, 100, 1000, 1200), buffer(1000.0, 1200.0))),
                "and at 2x the cut is the same tile in twice the pixels"
            );
        }

        /// **A client that fits is drawn exactly as it was before #133**:
        /// through no crop at all, at the factor `resizing::factor` gives.
        ///
        /// Which covers every pane that is not tiled -- a maximised, fullscreen
        /// or floating window arrives here with `tiled` false -- and a pane
        /// under a resize hold, which `Solium::tile_of` also answers `None`
        /// for. The hold's bridge is asserted here by its numbers: a 64-pixel
        /// buffer stretched into the 300x200 the drag has reached, with
        /// nothing cut.
        #[test]
        fn a_client_with_nothing_to_cut_is_not_cut() {
            let committed = Size::from((500, 600));
            for tiled in [true, false] {
                let settled = fit(
                    logical(100.0, 50.0, 500.0, 600.0),
                    AT_REST,
                    committed,
                    tiled,
                    None,
                );
                assert_eq!(
                    settled,
                    Fit {
                        factor: Scale::from((1.0, 1.0)),
                        crop: None,
                        shown: committed,
                    },
                    "tiled: {tiled}"
                );
                assert_eq!(
                    drawn(Surface::new((100, 50), (500, 600)), (100, 50), settled, 1.0),
                    Some(whole(physical(100, 50, 500, 600), buffer(500.0, 600.0)))
                );
            }

            let small = Size::from((64, 64));
            let held = fit(
                logical(400.0, 300.0, 300.0, 200.0),
                AT_REST,
                small,
                false,
                Some(crate::resizing::Fill::Stretch),
            );
            assert_eq!(
                held,
                Fit {
                    factor: Scale::from((300.0 / 64.0, 200.0 / 64.0)),
                    crop: None,
                    shown: small,
                },
                "a held pane is bridged into the dragged rectangle, not cut to it"
            );

            // And a window no layout tiles is squashed by a rectangle smaller
            // than its buffer, as every window was before #133: a dialog
            // gliding to a smaller rect, or one a script presents narrower.
            let free = fit(
                logical(0.0, 0.0, 250.0, 600.0),
                AT_REST,
                committed,
                false,
                None,
            );
            assert_eq!(free.factor, Scale::from((0.5, 1.0)));
            assert_eq!(free.crop, None, "nothing cuts a window that is in no tile");
        }

        /// **An animation scales the cut with the window; it does not cut the
        /// window.**
        ///
        /// Open, close and the overview reach here as a drawn rectangle that
        /// is a *picture* of the pane: smaller or larger than it by the
        /// frame's zoom. The cut is that drawn rectangle and the buffer is
        /// scaled by the same zoom, so what is inside the cut is the same part
        /// of the buffer at every size -- the first 500 of 1000 pixels, as
        /// settled. A cut left at the settled rectangle while the window shrank
        /// would cut the window; a factor taken from the committed size would
        /// squash it. Half size for an open or a thumbnail, and one and a half
        /// for a mode that enlarges.
        #[test]
        fn an_animated_tiled_client_is_scaled_with_its_cut_and_not_cut_by_it() {
            let committed = Size::from((1000, 600));

            let half = fit(
                logical(100.0, 50.0, 250.0, 300.0),
                (0.5, 0.5),
                committed,
                true,
                None,
            );
            assert_eq!(half.factor, Scale::from((0.5, 0.5)));
            assert_eq!(
                half.shown,
                Size::from((500, 600)),
                "the same share of the buffer as settled"
            );
            assert_eq!(
                drawn(Surface::new((100, 50), (1000, 600)), (100, 50), half, 1.0),
                Some(cut(physical(100, 50, 250, 300), buffer(500.0, 600.0))),
                "at half size the cut is half the tile and shows the same half of \
                 the buffer it shows settled"
            );

            let larger = fit(
                logical(100.0, 50.0, 750.0, 900.0),
                (1.5, 1.5),
                committed,
                true,
                None,
            );
            assert_eq!(larger.factor, Scale::from((1.5, 1.5)));
            assert_eq!(
                drawn(Surface::new((100, 50), (1000, 600)), (100, 50), larger, 1.0),
                Some(cut(physical(100, 50, 750, 900), buffer(500.0, 600.0))),
                "and enlarged it is the tile enlarged, not more of the buffer"
            );
        }

        /// **#133 review, finding 1: a glide that narrows a tiled window cuts
        /// it and does not zoom it.**
        ///
        /// One full-width window, 1900 wide, and a second one opening: the
        /// layout halves the first to 950 and it glides there while its
        /// client still has 1900 committed. A frame of that glide is drawn at
        /// a rectangle the window is passing through -- 1720 wide, say, at
        /// zoom 1.0 -- and it is **not** a scale of the new tile. What was
        /// shipped read it as one: the factor was the drawn size over the
        /// tile's share, 1720 / 950 = 1.81, so the left 950 pixels of buffer
        /// were stretched over 1720 of screen on the first frame after the
        /// sweep, where the frame before had drawn all 1900 of them 1:1.
        ///
        /// Here the buffer stays 1:1 and the cut follows the glide: 1720 of
        /// screen showing the first 1720 pixels of buffer. The first frame of
        /// the glide is the rectangle the window had, which cuts nothing and
        /// is the frame before it exactly; the last is the settled window.
        ///
        /// And the other way: a client 600 wide held in a 500 tile, whose
        /// tile grows to 700. On the first frame the drawn rect is the old
        /// tile, and the picture is the same 500-pixel cut it was a frame ago
        /// -- not the whole 600 pressed into 500 (0.83), which is what reading
        /// that rect as a scale of the new tile made of it.
        #[test]
        fn a_glide_that_narrows_a_tiled_window_cuts_it_and_does_not_zoom_it() {
            let committed = Size::from((1900, 1000));

            let first = fit(
                logical(0.0, 0.0, 1900.0, 1000.0),
                AT_REST,
                committed,
                true,
                None,
            );
            assert_eq!(
                first,
                Fit {
                    factor: Scale::from((1.0, 1.0)),
                    crop: None,
                    shown: committed,
                },
                "the first frame is the frame before the sweep"
            );

            let early = fit(
                logical(0.0, 0.0, 1720.0, 1000.0),
                AT_REST,
                committed,
                true,
                None,
            );
            assert_eq!(
                early.factor,
                Scale::from((1.0, 1.0)),
                "not zoomed: this is the window on its way, at its own size"
            );
            assert_eq!(early.shown, Size::from((1720, 1000)));
            assert_eq!(
                drawn(Surface::new((0, 0), (1900, 1000)), (0, 0), early, 1.0),
                Some(cut(physical(0, 0, 1720, 1000), buffer(1720.0, 1000.0))),
                "1720 pixels of screen, showing the first 1720 pixels of buffer"
            );

            let landed = fit(
                logical(0.0, 0.0, 950.0, 1000.0),
                AT_REST,
                committed,
                true,
                None,
            );
            assert_eq!(
                drawn(Surface::new((0, 0), (1900, 1000)), (0, 0), landed, 1.0),
                Some(cut(physical(0, 0, 950, 1000), buffer(950.0, 1000.0))),
                "and it lands on the settled window, cut to its tile"
            );

            let grown = fit(
                logical(0.0, 0.0, 500.0, 400.0),
                AT_REST,
                Size::from((600, 400)),
                true,
                None,
            );
            assert_eq!(
                grown.factor,
                Scale::from((1.0, 1.0)),
                "the first frame of a tile growing under an oversized client is \
                 the cut it already had"
            );
            assert_eq!(
                drawn(Surface::new((0, 0), (600, 400)), (0, 0), grown, 1.0),
                Some(cut(physical(0, 0, 500, 400), buffer(500.0, 400.0)))
            );
        }

        /// **#133 review, finding 11: a glide between tiles of different
        /// shapes is fitted one axis at a time.**
        ///
        /// The most common glide there is: a sibling squeezed sideways when a
        /// window opens, its height unchanged. Each axis gets its own answer:
        ///
        /// * Before the client answers, the width the glide has reached is
        ///   narrower than the 1900 still committed -- cut, 1:1 -- and the
        ///   height is the height it had: nothing is stretched.
        /// * Once the client has answered with the new tile's 950, the glide is
        ///   wider than the buffer and stretches it into the rectangle, on that
        ///   axis alone, which is what every glide did before #133 once its
        ///   client had committed. Nothing is cut.
        /// * And a client that is short on one axis and oversized on the other
        ///   is cut on the one and stretched on the other.
        #[test]
        fn a_glide_between_tiles_of_different_shapes_is_fitted_per_axis() {
            let client = logical(0.0, 0.0, 1400.0, 1000.0);

            let unanswered = fit(client, AT_REST, Size::from((1900, 1000)), true, None);
            assert_eq!(unanswered.factor, Scale::from((1.0, 1.0)));
            assert_eq!(unanswered.crop, Some(client));
            assert_eq!(unanswered.shown, Size::from((1400, 1000)));

            let answered = fit(client, AT_REST, Size::from((950, 1000)), true, None);
            assert_eq!(
                answered.factor,
                Scale::from((1400.0 / 950.0, 1.0)),
                "stretched across and not down"
            );
            assert_eq!(answered.crop, None, "and nothing to cut");

            let mixed = fit(client, AT_REST, Size::from((1900, 800)), true, None);
            assert_eq!(mixed.factor, Scale::from((1.0, 1000.0 / 800.0)));
            assert_eq!(mixed.crop, Some(client));
            assert_eq!(
                mixed.shown,
                Size::from((1400, 800)),
                "cut across, the whole of it down"
            );
        }

        /// **A surface the cut leaves nothing of is dropped**, rather than
        /// handed on as an element of no size: `CropRenderElement` answers
        /// `None` for it, and `fitted` passes that through for the caller to
        /// skip. A subsurface lying wholly past the tile's right edge is one.
        #[test]
        fn a_surface_wholly_past_the_tile_is_dropped() {
            let settled = fit(
                logical(100.0, 50.0, 500.0, 600.0),
                AT_REST,
                Size::from((1000, 600)),
                true,
                None,
            );
            assert_eq!(
                drawn(Surface::new((700, 50), (200, 100)), (100, 50), settled, 1.0),
                None
            );
            assert!(
                drawn(Surface::new((550, 50), (200, 100)), (100, 50), settled, 1.0)
                    .is_some_and(|shown| shown.cut && shown.geometry == physical(550, 50, 50, 100)),
                "one that straddles the edge keeps the part inside it"
            );
        }

        /// **A popup reaches past its parent's tile.**
        ///
        /// A menu is its own window: cut to its parent's tile it would lose
        /// every item past the tile's edge, and one opened from the right of a
        /// narrow tile is mostly past it. So `elements` puts a popup through
        /// the parent's fit with the cut taken off -- scaled with its window,
        /// never cut. The first half shows the same popup *would* be cut by the
        /// parent's own fit, so the second half cannot pass by accident.
        ///
        /// The second assertion is about the source text, for the reason the
        /// drag-icon test gives: `elements` cannot be run here. It pins only
        /// that the popup loop asks for `uncut`. That no *other* copy of the
        /// popup reaches the element list -- smithay's whole-window call drew
        /// one, cut with the toplevel -- is
        /// `state::tests::real_client::a_toplevel_is_drawn_without_its_popups`.
        #[test]
        fn a_popup_reaches_past_its_parents_tile() {
            let parent = fit(
                logical(100.0, 50.0, 500.0, 600.0),
                AT_REST,
                Size::from((1000, 600)),
                true,
                None,
            );
            let menu = || Surface::new((550, 80), (200, 300));

            assert_eq!(
                drawn(menu(), (100, 50), parent, 1.0).map(|shown| shown.geometry),
                Some(physical(550, 80, 50, 300)),
                "through the parent's own fit the menu would keep 50 of its 200 pixels"
            );
            assert_eq!(
                drawn(menu(), (100, 50), parent.uncut(), 1.0),
                Some(whole(physical(550, 80, 200, 300), buffer(200.0, 300.0))),
                "and through the fit popups are given, all of it is drawn"
            );

            let source = include_str!("render.rs");
            let popups = source
                .find("let (popups, covered) =")
                .expect("`elements` still draws the popups");
            let sandwich = source[popups..]
                .find("// **This is the sandwich.**")
                .map(|at| popups + at)
                .expect("and the sandwich still follows them");
            assert!(
                source[popups..sandwich].contains("fitting.uncut()"),
                "the popups in `elements` no longer draw through the uncut fit"
            );
        }
    }

    /// **The hotspot comes off the pointer's position, it is not added to it.**
    ///
    /// Both pictures the pointer carries are placed by [`origin_at`] — the
    /// cursor surface a client set, and now the icon a client attached to a
    /// drag (#57). A sign error here does not look like an offset: a 24-pixel
    /// I-beam whose hotspot is its middle would be drawn a whole image below
    /// and right of the text it is between, and a drag icon would trail the
    /// cursor instead of sitting under it.
    #[test]
    fn a_hotspot_moves_the_picture_up_and_left_of_the_pointer() {
        let pointer = smithay::utils::Point::<f64, smithay::utils::Logical>::from((100.0, 200.0));

        assert_eq!(
            origin_at(pointer, (0, 0).into()),
            smithay::utils::Point::from((100, 200)),
            "a picture whose hot point is its own corner sits exactly at the \
             pointer -- which is where the protocol puts a drag icon, since \
             nothing ever gives one a hotspot"
        );
        // Rules out the addition: that would answer (112, 212).
        assert_eq!(
            origin_at(pointer, (12, 12).into()),
            smithay::utils::Point::from((88, 188)),
            "a hot point twelve pixels into the image puts the image's corner \
             twelve pixels up and left, so the hot point lands on the pointer"
        );
        // A negative hotspot is legal -- `wl_pointer.set_cursor` takes plain
        // signed integers and a client may name a point outside its own
        // surface -- so the arithmetic is asserted in that direction too rather
        // than only on the half that a clamp would also pass.
        assert_eq!(
            origin_at(pointer, (-5, 5).into()),
            smithay::utils::Point::from((105, 195))
        );
        // The rounding is the pointer's, and it is `round` rather than a
        // truncation: a pointer halfway between two pixels belongs to the
        // nearer one, and truncating would bias every sub-pixel position of
        // every cursor and every drag icon towards the origin.
        assert_eq!(
            origin_at((99.6, 199.4).into(), (0, 0).into()),
            smithay::utils::Point::from((100, 199))
        );
    }

    /// **A drag icon must not be drawn over a locked screen, and what stops it
    /// is where its call sits in [`super::elements`].**
    ///
    /// This is a test of the source text, which is not how anything else here
    /// is checked and wants justifying. The behaviour cannot be reached from a
    /// unit test: `elements` needs a `GlesRenderer`, which needs a GPU that the
    /// build container does not have and that `cargo test` has no way to
    /// stand up. The alternative was a second `if state.lock.is_some()` inside
    /// the drag-icon path, which *would* be testable -- and which is exactly
    /// the shape of guard this compositor has already been bitten by, because
    /// two guards drift and the one that is forgotten is the one that leaks a
    /// client's pixels onto a lock screen. See the guard's own comment in
    /// `input::pointer_button` for the same argument from the other side.
    ///
    /// So the single guard stays the early return, and the claim that the icon
    /// is below it is pinned here instead of being left to a reader. What this
    /// proves is only the ordering of two statements; it would not notice a
    /// third path that drew the icon from somewhere else entirely.
    #[test]
    fn the_drag_icon_is_emitted_below_the_lock_screens_early_return() {
        let source = include_str!("render.rs");

        let lock = source
            .find("if let Some(lock) = state.lock.as_ref() {")
            .expect("`elements` still guards the locked session");
        // The early return *inside* that block, rather than the block's start:
        // what matters is that the icon is unreachable once the lock has
        // returned, not merely that it is written further down the file.
        let returned = source[lock..]
            .find("return elements;")
            .map(|at| lock + at)
            .expect("the lock guard still returns the frame it has built");
        let icon = source
            .find("elements.extend(drag_icon(")
            .expect("`elements` still draws the drag icon");

        assert!(
            icon > returned,
            "the drag icon is emitted at byte {icon}, above the lock screen's \
             early return at {returned} -- a locked session would draw a \
             client's surface over the lock screen"
        );
    }

    /// A QML scene's two answers, with Qt's exact behaviour.
    ///
    /// Every field is a fact about the real host rather than a convenience:
    ///
    /// * `dirty` is `SoliumQmlScene::dirty` in `qml/host.cpp`. Qt raises it
    ///   from `renderRequested` and `sceneChanged` — which fire when a property
    ///   the scene *renders* changes value, and not otherwise.
    /// * a draw **clears** it — `solium_qml_scene_render` and the GPU path both
    ///   end with `scene->dirty = false`. That is the whole mechanism: the flag
    ///   is spent by drawing, so there is exactly one moment at which it can be
    ///   read, and it is before.
    /// * `steps` is the animation itself: one entry per remaining tick, saying
    ///   whether *that* tick moves something the scene renders. It is a list
    ///   and not a countdown because the two are not the same shape, and
    ///   assuming they were is the defect this file now carries a test for: an
    ///   animation ticks on every frame and only sometimes changes a pixel.
    ///   This stand-in previously read `dirty |= running`, which is that wrong
    ///   assumption written down, and it is why the first fix here passed its
    ///   own tests with the bug still in it.
    /// * `solium_qml_scene_animating` is `!steps.is_empty()` — an animation is
    ///   running for as long as it has ticks left, whatever they do.
    ///
    /// The measured shape from Qt 6.11.2, for a settled decoration on the frame
    /// the compositor writes the property that triggers it and the three ticks
    /// after (`dirty`/animation running):
    ///
    /// | scene | write | +1 | +2 | +3 |
    /// |---|---|---|---|---|
    /// | `reveal.qml` | `false`/yes | `false`/yes | `true`/yes | `true`/yes |
    /// | `reactive.qml` | `false`/yes | `true`/yes | `true`/yes | `true`/yes |
    /// | `top.qml` | `true`/yes | `false`/yes | `true`/yes | `true`/yes |
    /// | `border.qml` | `true`/yes | `false`/yes | `true`/yes | `true`/yes |
    /// | `proximity.qml` | `true`/yes | `true`/yes | `true`/yes | `true`/yes |
    ///
    /// That table used to end `true`/**no** on every row, and re-measuring it is
    /// the only reason this comment changed: not one of these animations is
    /// shorter than 100ms, `reveal.qml`'s is 260ms, and none of them can be over
    /// three ticks — 48ms — after it started. They read `no` because they had
    /// already been advanced past their own end in one step, by an animation
    /// clock that handed each newly registered animation the compositor's whole
    /// uptime (`CompositorAnimationDriver` in `qml/host.cpp` has the mechanism).
    ///
    /// Which is worth saying here, in the stand-in, because **the stand-in
    /// cannot express that defect and should not be made to**. `dirty` and
    /// `animating` were both correct throughout it; the loop below asked for
    /// exactly the right frames; every frame drawn showed the finished value.
    /// A model of two booleans has no room for "at what rate", and a model
    /// extended until it had room would be a model of Qt's `QUnifiedTimer`
    /// written in Rust and kept true by hand — which is the same mistake as the
    /// `dirty |= running` line above, one layer further out.
    /// `dev/wirecheck`'s appear case asks the rate question of a real Qt.
    struct Scene {
        dirty: bool,
        steps: std::collections::VecDeque<bool>,
        draws: u32,
    }

    impl Scene {
        /// A scene whose animation moves something on the ticks that are
        /// `true` and nothing on the ticks that are `false`.
        fn animating(steps: &[bool]) -> Self {
            Self {
                dirty: false,
                steps: steps.iter().copied().collect(),
                draws: 0,
            }
        }

        /// An animation that moves the picture on every one of its ticks — the
        /// easy case, and the only one the previous stand-in could express.
        fn moving(ticks: usize) -> Self {
            Self::animating(&vec![true; ticks])
        }

        /// One `qml::tick`: the driver steps every animation in the process,
        /// and this one marks the scene dirty only if the step it took changed
        /// something that gets rendered.
        fn tick(&mut self) {
            if let Some(moved) = self.steps.pop_front() {
                self.dirty |= moved;
            }
        }

        /// One render, which is what spends the flag.
        const fn draw(&mut self) {
            self.dirty = false;
            self.draws += 1;
        }

        fn settled(&self) -> bool {
            self.steps.is_empty()
        }
    }

    impl Painted for Scene {
        fn something_new_to_draw(&self) -> bool {
            self.dirty
        }

        fn animation_in_flight(&self) -> bool {
            !self.steps.is_empty()
        }
    }

    /// One compositor frame over one scene, in the order the backends run it.
    ///
    /// `render::prepare` ticks every animation in the process; `render::elements`
    /// then draws the scenes and each draw says whether its scene still has
    /// somewhere to go. Returns what `chrome` does with that answer, which is
    /// `state.redraw` — whether there will *be* a next frame.
    fn one_frame(scene: &mut Scene) -> bool {
        // render::prepare
        scene.tick();
        // render::elements -> chrome -> Decoration::frame
        Drawn::drawing(scene, |scene| {
            scene.draw();
            None
        })
        .animating
    }

    /// Run the compositor's loop until it goes idle, or give up.
    ///
    /// The loop and not a frame, because the defect is a loop that stops: a
    /// frame happens only because the frame before it asked for one, and
    /// nothing else on screen is damaging anything. `None` means it never went
    /// idle, which is its own failure and the one the rejected fix produced.
    fn until_idle(scene: &mut Scene, limit: u32) -> Option<u32> {
        let mut frames = 0;
        while frames < limit {
            frames += 1;
            if !one_frame(scene) {
                return Some(frames);
            }
        }
        None
    }

    /// **An animating scene keeps asking for the frame after it.**
    ///
    /// The first regression. A decoration animates on its own clock and damages
    /// nothing, so the only thing that brings the next frame is this answer.
    /// Read after the draw it is always false — the draw has just cleared it —
    /// and the animation then advances only when something *else* happens to
    /// damage the screen. On the hardware that is a pulse and a hover tooltip
    /// that run while the mouse is moving and stop dead the instant it stops;
    /// nested, with nothing else on screen, it was zero frames in sixty
    /// seconds.
    ///
    /// Sixty frames rather than one, because one frame cannot tell a loop that
    /// keeps going from a loop that stops after the first.
    #[test]
    fn an_animating_scene_asks_for_the_frame_after_it() {
        let mut scene = Scene::moving(60);
        for step in 1..=60_u32 {
            assert!(
                one_frame(&mut scene),
                "frame {step} did not ask for another: the animation stops here"
            );
        }
        assert_eq!(scene.draws, 60, "a frame was asked for and not drawn");
    }

    /// **A tick that changes no pixels still has to bring the next frame.**
    ///
    /// The residual, and the case that survived the fix above because nothing
    /// could express it: the old stand-in raised `dirty` on every tick of a
    /// running animation, which is not what Qt does. Qt raises it on a
    /// *change*, and an animation produces none on the tick that starts it —
    /// the `Behavior` has begun and the property has not moved — nor on any
    /// tick whose interpolated value lands back on the one already there.
    ///
    /// The pattern here is `reveal.qml`'s, measured: clean ticks and then one
    /// that moves. On the dirty flag alone the loop stops on the first of them
    /// and the bar never slides out at all; it appears only if the pointer
    /// happens to keep moving, which is why it was "sometimes".
    ///
    /// Where those clean ticks come from, since the first reading of them was
    /// wrong and the wrong reading is the more plausible one: they are not an
    /// easing curve rounding to the value it already had. Starting a QML
    /// animation does not register it on the spot —
    /// `QAnimationTimer::registerAnimation` queues `startAnimations` through the
    /// event loop (qtbase v6.11.2, `qabstractanimation.cpp:659`) — so it is
    /// `running` immediately and advancing only from the tick after the
    /// compositor next drains Qt's queue. `reveal.qml` measures two such ticks
    /// with the queue drained once a frame; the three here are deliberately not
    /// a transcript, because the count is the one part of this that is not a
    /// property of the compositor — it follows from the drain rate — and a test
    /// pinned to it would fail on a faster screen for no reason. What has to
    /// hold is that a clean tick, however many there are, still brings the next
    /// frame.
    ///
    /// Nothing is wrong with that head of clean ticks and nothing here should
    /// try to shorten it; it is worth naming only so the next person does not
    /// read a clean tick as evidence of an easing curve, which is what the
    /// first reading of this did.
    #[test]
    fn a_tick_that_moves_nothing_still_asks_for_the_frame_after_it() {
        let mut scene = Scene::animating(&[false, false, false, true]);
        let frames = until_idle(&mut scene, 100);
        assert!(
            scene.settled(),
            "the loop stopped after {frames:?} frames with {} ticks of the animation left: \
             it is frozen there for good, because nothing else is going to damage the screen",
            scene.steps.len()
        );
        assert_eq!(
            frames,
            Some(5),
            "four ticks of animation and one to notice it is over"
        );
    }

    /// And a quiet stretch in the *middle* of one, which is the same defect
    /// wherever it lands: a colour easing between two nearby values spends
    /// several ticks rounding to the value it already had.
    #[test]
    fn a_quiet_stretch_in_the_middle_does_not_end_the_animation() {
        let mut scene = Scene::animating(&[true, true, false, false, false, false, true, true]);
        until_idle(&mut scene, 100);
        assert!(
            scene.settled(),
            "the animation stopped {} ticks short, mid-way through",
            scene.steps.len()
        );
    }

    /// **And it stops asking when the animation is over.**
    ///
    /// One frame later than the animation ends, and that is the right answer
    /// rather than a tolerated one: the tick that marks nothing is the first
    /// evidence there was nothing left to mark. One spare frame at the end of
    /// an animation costs a redraw; the alternative — deciding a frame early —
    /// is an animation that never draws its last step.
    #[test]
    fn a_settled_scene_stops_asking_one_frame_later() {
        let mut scene = Scene::moving(1);
        assert_eq!(
            until_idle(&mut scene, 100),
            Some(2),
            "a scene with nothing left to do kept the compositor drawing"
        );
        assert_eq!(scene.draws, 2);
    }

    /// **An animation that ends must let the compositor sleep.**
    ///
    /// The risk the fix above creates, and the reason the obvious answer was
    /// not taken. `QAnimationDriver::isRunning()` reads like "does Qt have a
    /// running animation" and is not: `advanceAnimation` ends in
    /// `QUnifiedTimer::localRestart`, which starts the driver again whenever it
    /// is not running and no animation is registered at all (qtbase v6.11.2,
    /// `src/corelib/animation/qabstractanimation.cpp:333`). Measured against
    /// this Qt: gated on `isRunning()`, with the screen damaged for eight
    /// frames after the animation started — a pointer still moving, which is
    /// usually *why* it started — the loop drew 400 frames of 400 and never
    /// went idle. A stuck animation traded for a compositor that never sleeps
    /// is a worse bug, not a fix.
    ///
    /// So: within two frames of the last tick, and never more.
    #[test]
    fn an_animation_that_ends_stops_the_loop() {
        for ticks in [1_usize, 2, 8, 60] {
            let mut scene = Scene::moving(ticks);
            let frames = until_idle(&mut scene, 10_000);
            assert_eq!(
                frames,
                Some(u32::try_from(ticks).unwrap_or(u32::MAX) + 1),
                "an animation of {ticks} ticks did not let the loop go idle right after it"
            );
        }
    }

    /// And a frame drawn for some unrelated reason, once it is over, must not
    /// start the loop up again.
    ///
    /// The latch test. Anything that answers "is this animating" from
    /// process-wide state rather than from this scene passes every test above
    /// and fails this one, because the pointer moving over a window is exactly
    /// the frame that would re-arm it.
    #[test]
    fn a_frame_drawn_for_another_reason_does_not_restart_a_finished_animation() {
        let mut scene = Scene::moving(3);
        assert!(until_idle(&mut scene, 100).is_some());
        assert!(
            !one_frame(&mut scene),
            "a frame the pointer asked for left the compositor asking for more"
        );
    }

    /// **The client is drawn between two layers the same style produced.**
    ///
    /// The claim this whole feature exists for, and an ordering claim — so the
    /// evidence is the list itself. `pane_pieces` is the walk the compositor
    /// runs, `PANE_ORDER` is the order it runs it in, and `Decoration` here is a
    /// real one: the `tests/fixtures/panes/example/` bundle, read by the real
    /// `style::load` and built into three real Qt scenes by
    /// `Decoration::from_style`. What is stood in for is the *element*, because
    /// making one needs a `GlesRenderer` and `cargo test` has no GPU — so the
    /// closure records a layer's name where the compositor's records a texture.
    ///
    /// Everything that can be wrong about the order is in the part that runs
    /// here: which depth goes first, which layers a depth has, and where the
    /// client falls among them.
    ///
    /// The control is `PANE_ORDER` itself. Moving `Piece::Client` to the front
    /// gives `["<client>", "spikes", "bar", "glow"]` and fails on the first
    /// assertion; swapping `Above` and `Behind` gives `["glow", "bar",
    /// "<client>", "spikes"]` and fails on the last two, which are there
    /// because the flat list alone does not say which side of the client each
    /// name was supposed to be on.
    #[test]
    fn a_client_is_drawn_between_two_layers_of_its_own_style() {
        on_the_qt_thread(|| {
            let dir = std::path::Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/panes/example"
            ));
            let style = crate::style::load(dir).expect("the example fixture loads");
            let decoration =
                crate::decoration::Decoration::from_style(&style, 300, 200).expect("three scenes");

            let mut order: Vec<&str> = Vec::new();
            super::pane_pieces(&mut order, |into, piece| match piece {
                super::Piece::Layers(depth) => into.extend(decoration.layers_at(depth)),
                super::Piece::Client => into.push("<client>"),
                super::Piece::Slot(..) => into.push("<slot>"),
            });

            assert_eq!(
                order,
                ["spikes", "bar", "<client>", "glow"],
                "topmost first: `above`, `frame`, the client, `behind`"
            );

            // Said again as the relation, because the flat list above is also
            // satisfied by an order that happens to spell the same names.
            let place = |name| {
                order
                    .iter()
                    .position(|each| *each == name)
                    .expect("every layer of the example is in the list")
            };
            assert!(
                place("spikes") < place("<client>"),
                "`above` must be over the client -- it is the half of this that \
                 a single decoration file could never do"
            );
            assert!(
                place("<client>") < place("glow"),
                "`behind` must be under the client, which is the other half"
            );
            assert!(
                place("bar") < place("<client>"),
                "and a `frame` layer still covers the client, as a decoration \
                 always has"
            );
        });
    }

    /// The example fixture's `Decoration`, as
    /// `a_client_is_drawn_between_two_layers_of_its_own_style` builds it. On
    /// the Qt thread only, with no Wayland client in the test (the #99 rule).
    fn example() -> crate::decoration::Decoration {
        let dir = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/panes/example"
        ));
        let style = crate::style::load(dir).expect("the example fixture loads");
        crate::decoration::Decoration::from_style(&style, 300, 200).expect("three scenes")
    }

    /// The walk over `decoration` with these slots, as names.
    fn walked(
        decoration: &crate::decoration::Decoration,
        has: &dyn Fn(crate::effect::plan::PaneSlot, crate::effect::rules::Slot) -> bool,
    ) -> Vec<String> {
        let mut order: Vec<String> = Vec::new();
        super::pane_walk(&mut order, has, |into, piece| match piece {
            super::Piece::Layers(depth) => {
                into.extend(decoration.layers_at(depth).map(ToOwned::to_owned));
            }
            super::Piece::Client => into.push("<client>".to_owned()),
            super::Piece::Slot(part, slot) => into.push(format!("<{part:?} {slot:?}>")),
        });
        order
    }

    /// **With no slots the pane walk is `PANE_ORDER`**, exactly.
    #[test]
    fn with_no_slots_the_pane_walk_is_pane_order() {
        on_the_qt_thread(|| {
            assert_eq!(
                walked(&example(), &|_, _| false),
                ["spikes", "bar", "<client>", "glow"]
            );
        });
    }

    #[test]
    fn client_slots_bracket_the_client_and_replace_takes_its_place() {
        use crate::effect::plan::PaneSlot;
        use crate::effect::rules::Slot;
        on_the_qt_thread(|| {
            let decoration = example();
            assert_eq!(
                walked(&decoration, &|part, slot| part == PaneSlot::Client
                    && slot != Slot::Replace),
                [
                    "spikes",
                    "bar",
                    "<Client Front>",
                    "<client>",
                    "<Client Behind>",
                    "glow"
                ]
            );
            assert_eq!(
                walked(&decoration, &|part, slot| part == PaneSlot::Client
                    && slot == Slot::Replace),
                ["spikes", "bar", "<Client Replace>", "glow"]
            );
        });
    }

    /// **Pane slots are around the sandwich**; the popups are not in the
    /// walk at all (they go in ahead of it), so a pane's `front` is below them.
    #[test]
    fn pane_slots_are_around_the_sandwich_and_below_the_popups() {
        use crate::effect::plan::PaneSlot;
        use crate::effect::rules::Slot;
        on_the_qt_thread(|| {
            let decoration = example();
            assert_eq!(
                walked(&decoration, &|part, slot| part == PaneSlot::Pane
                    && slot != Slot::Replace),
                [
                    "<Pane Front>",
                    "spikes",
                    "bar",
                    "<client>",
                    "glow",
                    "<Pane Behind>"
                ]
            );
            assert_eq!(
                walked(&decoration, &|part, slot| part == PaneSlot::Pane
                    && slot == Slot::Replace),
                ["<Pane Replace>"]
            );
        });
    }

    /// **A wanted slot with nothing ready draws what no slot draws**: a
    /// `replace` with nothing ready draws the part, `behind` and `front`
    /// nothing, so the walk with every slot wanted and none ready is the walk
    /// with none wanted. This is the walk's half of \[16\] §5's every-failure
    /// test; that every real failure leaves its slot not ready is Task 24's.
    #[test]
    fn a_wanted_slot_with_nothing_ready_draws_what_no_slot_draws() {
        use crate::effect::plan::{Owner, PaneSlot, Slots};
        use crate::effect::rules::{Origin, RuleKey, Slot};
        on_the_qt_thread(|| {
            let pane = crate::pane::PaneId::from_raw(1);
            let key = RuleKey {
                origin: Origin::User,
                index: 0,
                generation: 1,
            };
            let mut slots = Slots::default();
            for part in [
                PaneSlot::Pane,
                PaneSlot::Client,
                PaneSlot::Titlebar,
                PaneSlot::Popups,
                PaneSlot::Layer(0),
                PaneSlot::Layer(1),
                PaneSlot::Layer(2),
            ] {
                for slot in [Slot::Behind, Slot::Front, Slot::Replace] {
                    slots.want(Owner::Pane(pane, part), slot, key);
                }
            }
            let decoration = example();
            let ready = super::slot_ready(&slots, pane);
            assert_eq!(
                walked(&decoration, &ready),
                walked(&decoration, &|_, _| false),
                "a wanted slot with nothing ready changed the walk"
            );
        });
    }

    /// **The titlebar's slots bracket the `bar` layer**, wherever its depth:
    /// in `rounded` the bar is at `behind`.
    #[test]
    fn titlebar_slots_bracket_the_bar_layer() {
        use crate::style::Depth;
        assert_eq!(
            super::titlebar_layer(&[(0, Depth::Frame, "border"), (1, Depth::Behind, "bar")]),
            Some(1)
        );
        assert_eq!(
            super::titlebar_layer(&[(0, Depth::Above, "spikes"), (1, Depth::Frame, "frame")]),
            Some(1),
            "no `bar`: the first frame layer"
        );
        assert_eq!(super::titlebar_layer(&[(0, Depth::Behind, "shadow")]), None);
    }

    /// **A part's radii are its mask's, at its scale**: a client's own
    /// corners, a whole pane's largest at every corner, none unrounded.
    #[test]
    fn a_parts_radii_are_its_masks_at_its_scale() {
        use solium_effects::fragment::Corners;
        let top = Corners {
            top_left: 8.0,
            top_right: 8.0,
            bottom_left: 2.0,
            bottom_right: 0.0,
        };
        assert_eq!(
            super::mask_radii(Some(top), false, 2.0),
            [16.0, 16.0, 4.0, 0.0]
        );
        assert_eq!(super::mask_radii(Some(top), true, 2.0), [16.0; 4]);
        assert_eq!(super::mask_radii(None, true, 2.0), [0.0; 4]);
    }

    /// **The titlebar's box is its band, padded, with its outer corners**:
    /// `top`'s 32 pixels across the window, rounded at the top only.
    #[test]
    fn the_titlebars_box_is_its_band_padded_with_its_outer_corners() {
        use solium_effects::fragment::Corners;
        let regions = crate::effect::mask::FromInsets {
            insets: crate::decoration::Insets {
                top: 32,
                ..crate::decoration::Insets::default()
            },
            outer: (400, 300).into(),
            radii: Corners::all(10.0),
        };
        assert_eq!(
            super::region_box(&regions, "titlebar", 2.0, 4),
            Some(crate::effect::plan::PartBox::around(
                (800, 64).into(),
                4,
                [20.0, 20.0, 0.0, 0.0]
            ))
        );
        assert_eq!(super::region_box(&regions, "shelf", 2.0, 4), None);
    }

    /// **A pane drawn at the pad has its client inside it**: its frame's
    /// drawing at the pad in logical pixels, its client's corner the insets
    /// further in; at `(0, 0)`, as a warp's capture has always drawn it.
    #[test]
    fn a_pane_drawn_at_the_pad_has_its_client_inside_it() {
        let insets = crate::decoration::Insets {
            top: 32,
            left: 2,
            ..crate::decoration::Insets::default()
        };
        let outer: smithay::utils::Size<i32, smithay::utils::Logical> = (400, 300).into();
        for (at, scale, client) in [
            ((0, 0), 1.0, (2, 32)),
            ((6, 6), 1.0, (8, 38)),
            ((12, 12), 2.0, (16, 76)),
        ] {
            let (drawing, origin) = super::pane_drawing_at(outer, insets, at.into(), scale);
            let corner = drawing.rect.loc.to_physical(scale);
            assert_eq!(
                (corner.x, corner.y),
                (f64::from(at.0), f64::from(at.1)),
                "the frame's corner at {scale}"
            );
            assert_eq!(drawing.rect.size, outer.to_f64());
            assert_eq!(origin, client.into(), "the client's corner at {scale}");
        }
    }

    /// **A layer's self capture keeps only that layer**: of every layer its
    /// depth draws, what the hook saw pushed between its own before and
    /// after; nothing for a layer that pushed nothing or was not drawn.
    #[test]
    fn one_layer_keeps_only_the_layer_it_names() {
        // Three layers drawn, 0, 2 and 3, the last pushing nothing; 1 dormant.
        fn draw(into: &mut Vec<char>, hook: &mut dyn FnMut(&mut Vec<char>, usize, bool)) {
            for (index, name) in [(0, Some('s')), (2, Some('b')), (3, None)] {
                hook(into, index, true);
                into.extend(name);
                hook(into, index, false);
            }
        }
        assert_eq!(super::only_layer(2, draw), Some('b'));
        assert_eq!(super::only_layer(0, draw), Some('s'));
        assert_eq!(super::only_layer(3, draw), None, "it pushed nothing");
        assert_eq!(super::only_layer(1, draw), None, "it was not drawn");
    }

    /// **A layer's capture puts its canvas's corner at the pad**, its bleed
    /// and all, at a fractional scale too: where `Decoration::layer_elements`
    /// places the layer drawn with [`super::layer_drawing_at`].
    #[test]
    fn a_layers_capture_puts_its_canvas_corner_at_the_pad() {
        let outer: smithay::utils::Size<i32, smithay::utils::Logical> = (300, 200).into();
        let bleed = crate::style::Bleed {
            top: 12,
            right: 0,
            bottom: 4,
            left: 20,
        };
        let canvas = crate::decoration::canvas(smithay::utils::Rectangle::from_size(outer), bleed);
        for (scale, pad) in [(1.0, 6), (2.0, 12), (1.25, 8)] {
            let drawing = super::layer_drawing_at(canvas, outer, (pad, pad).into(), scale);
            let spread = crate::decoration::spread(drawing, bleed);
            assert_eq!(spread.canvas, canvas);
            let corner = spread.drawn.loc.to_physical(scale);
            assert!(
                (corner.x - f64::from(pad)).abs() < 1e-9
                    && (corner.y - f64::from(pad)).abs() < 1e-9,
                "at {scale} the canvas's corner is at {corner:?}, not at the pad {pad}"
            );
            assert_eq!(
                spread.drawn.size,
                canvas.size.to_f64(),
                "drawn at its own size"
            );
        }
    }

    /// **Around a layer its own slots are outermost**, the titlebar's inside
    /// them on the titlebar's layer: a titlebar is a band of its layer. A
    /// layer's `replace` goes in its place; the titlebar's, a band, over it.
    #[test]
    fn around_a_layer_its_own_slots_are_outermost_and_the_titlebars_inside() {
        use super::Around::{After, Before, InPlace};
        use crate::effect::plan::PaneSlot::{Layer, Titlebar};
        use crate::effect::rules::Slot::{Behind, Front, Replace};
        assert_eq!(
            super::around_layer(1, Some(1)).collect::<Vec<_>>(),
            [
                (Layer(1), Front, Before),
                (Titlebar, Front, Before),
                (Titlebar, Replace, Before),
                (Layer(1), Replace, InPlace),
                (Titlebar, Behind, After),
                (Layer(1), Behind, After),
            ]
        );
        assert_eq!(
            super::around_layer(0, Some(1)).collect::<Vec<_>>(),
            [
                (Layer(0), Front, Before),
                (Layer(0), Replace, InPlace),
                (Layer(0), Behind, After),
            ],
            "not the titlebar's layer"
        );
    }

    /// **A part is bracketed by its `front` and `behind`, and replaced in its
    /// place**: what a layer surface, a scripted surface and a window's
    /// popups are walked by.
    #[test]
    fn a_part_is_bracketed_by_its_front_and_behind_and_replaced_in_its_place() {
        use crate::effect::rules::Slot;
        let walk = |has: &dyn Fn(Slot) -> bool| {
            let mut order = Vec::new();
            super::bracket(has, |slot| order.push(slot));
            order
        };
        assert_eq!(walk(&|_| false), [None], "the part alone");
        assert_eq!(
            walk(&|slot| slot != Slot::Replace),
            [Some(Slot::Front), None, Some(Slot::Behind)]
        );
        assert_eq!(walk(&|slot| slot == Slot::Replace), [Some(Slot::Replace)]);
    }

    /// **A layer's `replace` takes its place, and its other slots go around
    /// it**: `chrome`'s hook, driven as `Decoration::layer_elements` drives
    /// it, a layer's own elements pushed between the two calls.
    #[test]
    fn a_layers_replace_takes_its_place_and_its_other_slots_go_around_it() {
        use super::Around::{After, Before, InPlace};
        let drawn = |placed: &mut Vec<(usize, super::Around, Option<&'static str>)>| {
            let (mut into, mut mark) = (vec!["above"], 0);
            for (index, name) in [(0, "frame"), (1, "bar")] {
                super::place_around(placed, &mut mark, &mut into, index, true);
                into.push(name);
                super::place_around(placed, &mut mark, &mut into, index, false);
            }
            into
        };
        assert_eq!(
            drawn(&mut vec![
                (1, Before, Some("<front>")),
                (1, After, Some("<behind>")),
            ]),
            ["above", "frame", "<front>", "bar", "<behind>"]
        );
        assert_eq!(
            drawn(&mut vec![
                (1, Before, Some("<front>")),
                (1, InPlace, Some("<replace>")),
                (1, After, Some("<behind>")),
            ]),
            ["above", "frame", "<front>", "<replace>", "<behind>"]
        );
        assert_eq!(
            drawn(&mut Vec::new()),
            ["above", "frame", "bar"],
            "nothing ready"
        );
    }

    /// **The bleed cull counts an effect's reach**: a glow behind a pane
    /// reaching onto a monitor keeps the pane there.
    #[test]
    fn the_bleed_cull_counts_an_effects_reach() {
        let screen = smithay::utils::Rectangle::<i32, smithay::utils::Logical>::new(
            (0, 0).into(),
            (100, 100).into(),
        );
        let drawn = smithay::utils::Rectangle::<f64, smithay::utils::Logical>::new(
            (110.0, 0.0).into(),
            (50.0, 50.0).into(),
        );
        assert!(!super::reaches(drawn, 0, screen));
        assert!(super::reaches(drawn, 12, screen));
    }

    /// **A slot is placed over its part grown by its reach, as the part is
    /// drawn**: the padded box around the part's own rectangle, the mask the
    /// part inside it; a part drawn at half size has its reach halved too.
    #[test]
    fn a_slot_is_placed_over_its_part_grown_by_its_reach_as_the_part_is_drawn() {
        use smithay::utils::{Physical, Rectangle};
        use solium_effects::fragment::Corners;
        let radii = Corners::all(8.0);
        let at = |x: f64, y: f64, w: f64, h: f64| {
            Rectangle::<f64, Physical>::new((x, y).into(), (w, h).into())
        };
        let (dst, mask) =
            super::slot_placement(at(100.0, 50.0, 200.0, 100.0), (224, 124).into(), 12, radii);
        assert_eq!(dst, Rectangle::new((88, 38).into(), (224, 124).into()));
        assert_eq!(mask, (at(12.0, 12.0, 200.0, 100.0), radii));
        let (dst, mask) =
            super::slot_placement(at(100.0, 50.0, 100.0, 50.0), (224, 124).into(), 12, radii);
        assert_eq!(
            dst,
            Rectangle::new((94, 44).into(), (112, 62).into()),
            "half size"
        );
        assert_eq!(mask, (at(6.0, 6.0, 100.0, 50.0), radii));
    }

    /// **A result is cut by its part's shape unless it owns its edges**: an
    /// effect that reads `shape` draws its own (a glow outside the part), and
    /// `mask = "alpha"` is the self capture's, not the shape's (\[16\] §2).
    #[test]
    fn a_result_is_cut_by_its_parts_shape_unless_it_owns_its_edges() {
        use crate::effect::rules::MaskKind;
        assert!(super::cut_by_shape(MaskKind::Shape, false));
        assert!(
            !super::cut_by_shape(MaskKind::Shape, true),
            "it reads `shape`"
        );
        assert!(!super::cut_by_shape(MaskKind::Alpha, false));
    }

    #[test]
    fn a_window_at_real_size_is_not_scaled() {
        assert!((ratio(800.0, 800) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn half_size_is_half_scale() {
        assert!((ratio(400.0, 800) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn degenerate_sizes_fall_back_to_no_scaling() {
        assert!((ratio(400.0, 0) - 1.0).abs() < f64::EPSILON);
        assert!((ratio(0.0, 800) - 1.0).abs() < f64::EPSILON);
        assert!((ratio(-5.0, 800) - 1.0).abs() < f64::EPSILON);
    }

    /// What came out of [`by_depth`], as names, which is the only thing any of
    /// the depth tests asks about.
    fn names<'a>(order: &[(&'a str, f32)]) -> Vec<&'a str> {
        order.iter().map(|(name, _)| *name).collect()
    }

    /// Equal depths keep the order the stack gave them. This is the whole of
    /// the cheap path: every window is `0.0`, so the list must come out
    /// exactly as it went in, and an unstyled session pays nothing for a
    /// feature it does not use.
    ///
    /// Three distinct names, because a list of interchangeable payloads cannot
    /// see itself being permuted.
    ///
    /// **This list is already in its own sorted order, so the sort has nothing
    /// to move** — which is the property, and also why this is not on its own
    /// a test of stability. `raised_cards_sort_and_the_rest_keep_their_order`
    /// is the one where the sort does work and a tie is still observable
    /// afterwards.
    #[test]
    fn equal_depths_keep_the_stacking_order() {
        let mut order = vec![("a", 0.0_f32), ("b", 0.0), ("c", 0.0)];
        by_depth(&mut order);
        assert_eq!(names(&order), vec!["a", "b", "c"]);
    }

    /// And a raised node is drawn nearer the viewer — which, in a list the
    /// renderer walks topmost-first, means EARLIER.
    ///
    /// Three different depths, and an input order that is neither the answer
    /// nor the answer reversed: `under, over, between` sorts to `over,
    /// between, under` and reverses to `between, over, under`. So neither
    /// "leave it alone" nor "turn it round" passes, and neither does sorting
    /// the other way (`under, between, over`) nor sorting by name in either
    /// direction (`under, over, between` and `between, over, under`). Five
    /// wrong answers, five different lists.
    #[test]
    fn a_higher_depth_is_drawn_in_front() {
        let mut order = vec![("under", 0.0_f32), ("over", 2.0), ("between", 1.0)];
        by_depth(&mut order);
        assert_eq!(names(&order), vec!["over", "between", "under"]);
    }

    /// Raising cards leaves the rest of the stack in the order it was in.
    /// This is the case the default depth cannot show, and the one a script
    /// lifting a card actually asks for: the sort really has to move
    /// something, and the four untouched nodes have to come through it
    /// unshuffled.
    ///
    /// **Two raised cards at two different heights, and both heights non-zero**
    /// — the one arrangement the other three tests between them never reach.
    /// Without `middle`, every comparison the sort makes is against `0.0`, so a
    /// rule that ordered `0.0` correctly and `1.0` against `3.0` backwards
    /// would go unseen here. Four ties and two distinct lifts is the smallest
    /// fixture that asks both questions at once.
    ///
    /// A separate test from `equal_depths_keep_the_stacking_order` because the
    /// obvious wrong ways to write this both pass that one and
    /// `a_higher_depth_is_drawn_in_front`, and fail here:
    ///
    /// * repeatedly find the deepest remaining node and **swap** it into
    ///   place — the swap bringing `raised` to the front sends `top` to where
    ///   `raised` was, behind `second`;
    /// * find the deepest node and **rotate** it to the front, and stop —
    ///   which leaves `middle` where it lay, behind three nodes it now
    ///   outranks.
    #[test]
    fn raised_cards_sort_and_the_rest_keep_their_order() {
        let mut order = vec![
            ("top", 0.0_f32),
            ("second", 0.0),
            ("raised", 3.0),
            ("fourth", 0.0),
            ("fifth", 0.0),
            ("middle", 1.0),
        ];
        by_depth(&mut order);
        assert_eq!(
            names(&order),
            vec!["raised", "middle", "top", "second", "fourth", "fifth"]
        );
    }

    /// A NaN depth does not panic and does not move the window. A script
    /// reaches one with a single division, and the only answer a user can
    /// predict is that nothing happens — which is the same answer the default
    /// depth gives.
    ///
    /// This rejects both of the implementations that look right. `total_cmp`
    /// orders NaN above every finite float and sweeps the window to the front
    /// of the stack; `unwrap_or(Ordering::Less)` sweeps it to the front as
    /// well. Both were run.
    ///
    /// It does **not** reject `unwrap_or(Ordering::Greater)`, and that is
    /// worth writing down rather than leaving to be discovered: neither
    /// `Greater` nor `Equal` ever answers `Less` on a list of otherwise equal
    /// depths, so the sort sees a list already in order and hands it back
    /// untouched whichever of the two is written. `Equal` is here because it
    /// is the symmetric answer — the one that stays "leave it alone" when the
    /// depths around it are not equal — not because a test can see the
    /// difference on this fixture.
    ///
    /// For the same reason it cannot see the sort's *direction* either: every
    /// finite depth here ties, so an ascending `unwrap_or(Equal)` passes this
    /// one too. Direction is pinned by the two tests above, which is where it
    /// belongs; this test is about NaN and nothing else.
    #[test]
    fn a_depth_that_is_not_a_number_is_left_where_it_is() {
        let mut order = vec![("a", 0.0_f32), ("nan", f32::NAN), ("b", 0.0)];
        by_depth(&mut order);
        assert_eq!(names(&order), vec!["a", "nan", "b"]);
    }

    /// **A warped pane's capture walks its slots**: the pieces
    /// `flat_window_elements` builds from have the client's `replace` in the
    /// client's place, as the flat path's do. (`flat_window_elements` is
    /// concrete on `GlesRenderer`, so the order it walks is the testable half.)
    #[test]
    fn a_warped_panes_capture_walks_its_client_slot() {
        use crate::effect::plan::{Owner, PaneSlot, Slots};
        use crate::effect::rules::Slot;
        let pane = crate::pane::PaneId::from_raw(1);
        let mut slots = Slots::default();
        slots.mark_ready(Owner::Pane(pane, PaneSlot::Client), Slot::Replace);
        let pieces = super::capture_pieces(&slots, pane);
        assert!(
            pieces.contains(&super::Piece::Slot(PaneSlot::Client, Slot::Replace)),
            "{pieces:?}"
        );
        assert!(
            !pieces.contains(&super::Piece::Client),
            "the bare client is still in the capture"
        );
    }

    /// **A whole pane's self capture walks its inner results, not its own**:
    /// a `pane` effect reads the client's blur, and never its own last output.
    #[test]
    fn a_whole_panes_capture_walks_its_inner_slots_and_not_its_own() {
        use crate::effect::plan::{Owner, PaneSlot, Slots};
        use crate::effect::rules::Slot;
        let pane = crate::pane::PaneId::from_raw(1);
        let mut slots = Slots::default();
        slots.mark_ready(Owner::Pane(pane, PaneSlot::Client), Slot::Replace);
        slots.mark_ready(Owner::Pane(pane, PaneSlot::Pane), Slot::Behind);
        let pieces = super::inner_pieces(&slots, pane);
        assert!(
            pieces.contains(&super::Piece::Slot(PaneSlot::Client, Slot::Replace)),
            "{pieces:?}"
        );
        assert!(
            !pieces.contains(&super::Piece::Slot(PaneSlot::Pane, Slot::Behind)),
            "a pane's own slot is in its own capture: {pieces:?}"
        );
    }

    /// `prepare`'s phases, recorded: what ran in which order, and what the
    /// warp's capture walked when its job was built.
    struct Recorder {
        pane: crate::pane::PaneId,
        did: Vec<String>,
        walked: Vec<super::Piece>,
    }

    impl super::Phases for Recorder {
        fn compile(&mut self) {
            self.did.push("compile".to_owned());
        }
        fn resolve(&mut self) -> crate::effect::plan::Slots {
            use crate::effect::plan::{Owner, PaneSlot, Slots};
            use crate::effect::rules::{Origin, RuleKey, Slot};
            self.did.push("resolve".to_owned());
            let mut slots = Slots::default();
            slots.want(
                Owner::Pane(self.pane, PaneSlot::Client),
                Slot::Replace,
                RuleKey {
                    origin: Origin::User,
                    index: 0,
                    generation: 1,
                },
            );
            slots
        }
        fn captures(&mut self, _slots: &crate::effect::plan::Slots, nest: super::Nest) {
            self.did.push(format!("captures {nest:?}"));
        }
        fn chains(&mut self, slots: &mut crate::effect::plan::Slots, nest: super::Nest) {
            use crate::effect::plan::{Owner, PaneSlot};
            use crate::effect::rules::Slot;
            self.did.push(format!("chains {nest:?}"));
            // The client's chain ran this pass: its result is ready now.
            if nest == super::Nest::Inner {
                slots.mark_ready(Owner::Pane(self.pane, PaneSlot::Client), Slot::Replace);
            }
        }
        fn warps(&mut self, slots: &crate::effect::plan::Slots) {
            self.did.push("warps".to_owned());
            self.walked = super::capture_pieces(slots, self.pane);
        }
        fn sweep(&mut self) {
            self.did.push("sweep".to_owned());
        }
    }

    /// **`prepare` compiles first, and builds the warp and `over` jobs only
    /// after the chains have run** (Ruling 17; spec C14 for the first half):
    /// a warped pane's capture is built from a walk that already holds this
    /// pass's slot results, so its element list and its keyed `Inputs` carry
    /// the result's element, and a wanted effect has compiled before any slot
    /// resolves, so it is never absent from the first frame for want of one.
    #[test]
    fn prepare_compiles_first_and_builds_the_warps_after_the_chains() {
        use crate::effect::plan::PaneSlot;
        use crate::effect::rules::Slot;
        let mut recorder = Recorder {
            pane: crate::pane::PaneId::from_raw(1),
            did: Vec::new(),
            walked: Vec::new(),
        };
        let _ = super::effect_phases(&mut recorder);
        assert_eq!(
            recorder.did,
            [
                "compile",
                "resolve",
                "captures Inner",
                "chains Inner",
                "captures Whole",
                "chains Whole",
                "warps",
                "sweep"
            ]
        );
        assert!(
            recorder
                .walked
                .contains(&super::Piece::Slot(PaneSlot::Client, Slot::Replace)),
            "the warp's capture was built before the client's chain ran: {:?}",
            recorder.walked
        );
        assert!(
            !recorder.walked.contains(&super::Piece::Client),
            "the bare client is in the warp's capture"
        );
    }

    /// **A run phase with no chain of its own binds no carrier** (Ruling
    /// 17): a client's self chain runs in the inner phase, so the whole-pane
    /// phase has nothing to run and binds nothing, and with no slot neither
    /// does.
    #[test]
    fn a_run_phase_with_no_chain_of_its_own_binds_nothing() {
        use crate::effect::plan::{Chains, Owner, PaneSlot, Slots};
        use crate::effect::rules::{Origin, RuleKey, Slot};
        let place = crate::effect::host::tests::scratch("render-phase-binds");
        crate::effect::host::tests::folder(
            &place,
            "tint",
            "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        let mut host = crate::effect::host::Host::new(crate::effect::host::Library::with(
            Some(place.clone()),
            place.join("none"),
        ));
        host.want("rules", ["tint".to_owned()]);
        let lua = mlua::Lua::new();
        let value: mlua::Value = lua
            .load(r#"{ { match = "*", part = "client", slot = "replace", effect = "tint" } }"#)
            .eval()
            .expect("the test's Lua");
        let tree = crate::effect::tree::Tree::from_lua(&value)
            .expect("readable")
            .expect("a value");
        let rule = crate::effect::rules::parse(&tree)
            .expect("parses")
            .remove(0);
        let key = RuleKey {
            origin: Origin::User,
            index: 0,
            generation: 1,
        };
        let mut chains = Chains::default();
        chains.insert(key, Chains::bind(&mut host, &rule).expect("binds"));
        let mut slots = Slots::default();
        assert!(!super::runs_in(&slots, &chains, super::Nest::Inner));
        assert!(!super::runs_in(&slots, &chains, super::Nest::Whole));
        slots.want(
            Owner::Pane(crate::pane::PaneId::from_raw(1), PaneSlot::Client),
            Slot::Replace,
            key,
        );
        assert!(super::runs_in(&slots, &chains, super::Nest::Inner));
        assert!(
            !super::runs_in(&slots, &chains, super::Nest::Whole),
            "the whole-pane phase binds a carrier to run nothing"
        );
        let _ = std::fs::remove_dir_all(place);
    }
}
