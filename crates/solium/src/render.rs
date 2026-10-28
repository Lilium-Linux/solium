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
    /// A strip of a pane's background drawn through its client's rounded
    /// rectangle, so it stops where the client's corners do (#233). See
    /// [`background_strips`] and `crate::clip::ClippedSolid`.
    ClippedSolid = crate::clip::ClippedSolid,
    /// An effect's result in its slot, cut by its part's mask. See
    /// `crate::effect::element`.
    Effect = crate::effect::element::EffectElement,
    /// Two rules' results of one slot mixed in one draw as the slot
    /// crossfades from one to the other (#389). See
    /// `crate::effect::crossfade`.
    Crossfade = crate::effect::crossfade::CrossfadeElement,
    /// A slot's backdrop drawn beneath a warped pane through its mesh, each
    /// fragment reading it where it is on the monitor (Ruling F37). See
    /// `crate::effect::beneath`.
    Beneath = crate::effect::beneath::Beneath,
    /// The hole a warped pane's capture keeps where a slot is drawn beneath
    /// the warp. See `crate::effect::beneath`.
    Hole = crate::effect::beneath::Hole,
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
    /// Panes a refused `sol.present` geometry hides this pass
    /// (`effects.present.failed = "hide"`): nothing of them is drawn.
    /// `tests::a_refused_present_follows_its_failed_policy`.
    hidden: Vec<crate::pane::PaneId>,
    /// Every rule resolved once this pass ([`build_slots`]), so every output
    /// and every screencopy places from the same answer.
    pub(crate) slots: crate::effect::plan::Slots,
    /// The first frame's gate holds this pass ([`hold_first_frame`]):
    /// [`elements`] draws only the clear colour, on every monitor and in
    /// every screencopy. No unit test reaches `prepare` or `elements`:
    /// `dev/effects-check.sh xray`'s `xray-first-on`, under
    /// `SOLIUM_WARM_INJECT=3`, finds its first three frames only the clear
    /// colour and the fourth drawn, the plate's backdrop already blurred.
    pub(crate) held: bool,
    /// The pass's one instant ([`Presented::at`]), which [`elements`] walks
    /// at, so every output and screencopy draws where the T2 places were
    /// measured (amended 2026-10-14, live under motion and warps).
    /// `tests::the_walk_draws_at_the_passs_instant`.
    pub(crate) now: Option<std::time::Duration>,
}

/// One warp `prepare` made: the capture drawn through it, the program, the id
/// and commit its element carries, the part of the pane's unit square the
/// capture covers, and the grid a geometry effect placed over that part this
/// pass (`None`: the part where it is, as a tilt draws it).
/// `warp::tests::a_lua_grid_lands_where_the_rust_genie_put_it`.
#[derive(Clone, Debug)]
pub(crate) struct Warped {
    /// Its pane, which a window still loading is too
    /// (`state::tests::real_client::a_loading_pane_under_a_present_is_planned_through_the_warp`).
    pane: crate::pane::PaneId,
    texture: GlesTexture,
    program: crate::warp::Program,
    id: Id,
    commit: CommitCounter,
    part: crate::warp::UnitRect,
    grid: Option<crate::warp::Grid>,
    /// The slots drawn beneath it, on every monitor, in its capture's
    /// order ([`unders`]); none for its popups' warp.
    under: Vec<crate::effect::beneath::Under>,
    /// What it is drawn through in place of `program`: its present
    /// geometry's pixels, once they compiled ([`present_pixels`]).
    pixels: Option<crate::warp::Pixels>,
}

/// What a warp's mesh and its pixels are a function of, in global space, so
/// one comparison serves every output and screencopy. Exact comparison: a
/// `NaN` always differs, which recommits, the safe way round.
/// `keyed::tests::a_warp_whose_mesh_moves_inside_the_same_bounds_is_given_a_new_commit`,
/// `keyed::tests::a_pixels_only_warp_moves_its_commit_with_progress`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shape {
    rect: Rectangle<f64, Logical>,
    matrix: crate::mat4::Mat4,
    pivot: (f32, f32),
    scale: f64,
    aimed: Option<present::Aimed>,
    /// What its pixel program reads, if it is drawn through one: a warp
    /// whose mesh does not move still redraws as these do (Task 29).
    pixels: Option<crate::effect::gl::PixelUniforms>,
}

impl Shape {
    /// With no pixels: every warp that draws its capture plainly.
    pub(crate) fn of(frame: &present::Frame, aimed: Option<present::Aimed>, scale: f64) -> Self {
        Self {
            rect: frame.rect,
            matrix: frame.matrix,
            pivot: frame.pivot,
            scale,
            aimed,
            pixels: None,
        }
    }

    /// The same shape, drawn through a pixel program reading `pixels`.
    pub(crate) fn with_pixels(self, pixels: crate::effect::gl::PixelUniforms) -> Self {
        Self {
            pixels: Some(pixels),
            ..self
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

/// What a pane's warp capture holds past its outer rectangle: its style's
/// layers' bleed, all of them, a dormant layer's too, so one waking during
/// a flight is not cut and does not change the capture's size; none for a
/// bare pane. The same while its window loads as once its application has
/// arrived, so a present across that handover keeps its part and its grid.
/// A decoration needs Qt, so what is done with the bleed is what is tested
/// ([`pane_part`], `offscreen::pane_capture`).
pub(crate) fn capture_bleed(state: &Solium, pane: crate::pane::PaneId) -> crate::style::Bleed {
    state
        .panes
        .get(pane)
        .and_then(Pane::decoration)
        .map_or_else(
            crate::style::Bleed::default,
            crate::decoration::Decoration::bleed,
        )
}

/// `outer` grown by `bleed` on every side: the rectangle a warped pane's
/// capture holds (#312), its corner up and left of the pane's by the bleed.
/// Not `decoration::canvas`, which fits one layer's bleed under the largest
/// scene a layer may have: a capture holds every layer whole.
/// `tests::a_warped_panes_part_reaches_past_it_by_its_bleed`.
pub(crate) fn bled(
    outer: Rectangle<i32, Logical>,
    bleed: crate::style::Bleed,
) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (outer.loc.x - bleed.left, outer.loc.y - bleed.top).into(),
        (
            outer.size.w + bleed.left + bleed.right,
            outer.size.h + bleed.top + bleed.bottom,
        )
            .into(),
    )
}

/// The part of a pane's unit square its own warp covers: its outer
/// rectangle grown by `bleed` ([`bled`]), over the pane's `outer` size, so
/// a shadow or a title above the pane is captured, meshed and drawn with
/// it (#312), as its popups' part reaches past it ([`over_part`]).
/// Exactly [`crate::warp::UnitRect::WHOLE`] with no bleed.
/// `tests::a_warped_panes_part_reaches_past_it_by_its_bleed`,
/// `tests::a_pane_that_bleeds_nothing_is_captured_as_it_always_was`.
pub(crate) fn pane_part(
    outer: Size<i32, Logical>,
    bleed: crate::style::Bleed,
) -> crate::warp::UnitRect {
    if !bleed.any() {
        return crate::warp::UnitRect::WHOLE;
    }
    let (w, h) = (f64::from(outer.w.max(1)), f64::from(outer.h.max(1)));
    crate::warp::UnitRect {
        u0: -f64::from(bleed.left) / w,
        v0: -f64::from(bleed.top) / h,
        u1: 1.0 + f64::from(bleed.right) / w,
        v1: 1.0 + f64::from(bleed.bottom) / h,
    }
}

/// [`pane_part`] for `pane` as it is now: its slot's size and its
/// [`capture_bleed`]. What its grid is planned over, its capture taken at
/// and its warp drawn over, one answer for the three.
pub(crate) fn warp_part(state: &Solium, pane: crate::pane::PaneId) -> crate::warp::UnitRect {
    state
        .pane_outer_of(pane)
        .map_or(crate::warp::UnitRect::WHOLE, |outer| {
            pane_part(outer.size, capture_bleed(state, pane))
        })
}

/// Where the pane's own rectangle is inside a capture of `part` of its unit
/// square, its corner and its size in the capture's `uv`: a present's
/// pixels' `sol_content`, so a frag finds the pane inside a capture that
/// holds its layers' bleed (#312). `[0, 0, 1, 1]` for the whole pane.
/// `tests::sol_content_names_the_pane_inside_its_capture`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "what a shader reads, as f32"
)]
pub(crate) fn content_in(part: crate::warp::UnitRect) -> [f32; 4] {
    let pane = crate::warp::UnitRect::WHOLE.in_part(part);
    [
        pane.u0 as f32,
        pane.v0 as f32,
        (pane.u1 - pane.u0) as f32,
        (pane.v1 - pane.v0) as f32,
    ]
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

/// `present_source`, for a geometry already in hand.
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

/// What a present geometry's warp draws a piece's capture through, the
/// piece the `part` of the pane's unit square captured at `size`, its
/// subject at `content` inside it: the program its version's pixels
/// compiled into, once compiled, reading the geometry's progress with a
/// leaving's direction (as its `mesh` is told), its seed, the pass's time
/// when the frag reads it, the subject, the part, and its pixels' params.
/// The pane's own subject is the pane inside a capture that may hold its
/// layers' bleed ([`content_in`]); its popups' is their whole capture. None
/// for a geometry with no pixels or before they compiled, which the plain
/// warp draws.
/// `state::tests::real_client::a_present_draws_its_folders_frag_as_its_pixels`,
/// `state::tests::real_client::a_present_through_a_pixels_effect_reads_the_params_it_blends`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "what a shader reads, as f32"
)]
pub(crate) fn present_pixels(
    state: &Solium,
    pane: crate::pane::PaneId,
    geometry: present::Geometry,
    (part, content): (crate::warp::UnitRect, [f32; 4]),
    size: Size<i32, Physical>,
) -> Option<crate::warp::Pixels> {
    let loaded = source_of(state, pane, geometry)?;
    let pixels = loaded.pixels.as_ref()?;
    let program = state.effects.program(pixels.key)?.clone();
    // Read only by a frag that reads it, so a still warp whose frag does
    // not is not given a new commit every pass.
    let time = if program.reads("sol_time") {
        state.clock.now().as_secs_f32()
    } else {
        0.0
    };
    let uniforms = crate::effect::gl::PixelUniforms {
        progress: geometry.progress as f32,
        clamped: geometry.progress.clamp(0.0, 1.0) as f32,
        direction: -1.0,
        seed: geometry.seed as f32,
        time,
        size: [size.w as f32, size.h as f32],
        content,
        part: [
            part.u0 as f32,
            part.v0 as f32,
            part.u1 as f32,
            part.v1 as f32,
        ],
        params: geometry.pixels,
    };
    Some((program, uniforms, pixels.names.clone()))
}

/// The resolutions a warped pane's capture is taken at, as fractions of its
/// full one at its monitor's scale. A window flying to its icon crosses each
/// once, so it is captured again only there and at the commits and fades it
/// would be captured again for anyway, each a fraction of the pixels (#307).
const CAPTURE_STEPS: [f64; 7] = [1.0, 0.75, 0.5, 0.375, 0.25, 0.1875, 0.125];

/// How much of its full resolution a warped pane's capture needs this pass:
/// the most its present magnifies any part of it on screen against the
/// pane's own `outer` size, stepped up to one of [`CAPTURE_STEPS`].
///
/// A geometry's grid is measured cell by cell, so a mesh that stretches one
/// corner keeps the pixels that corner shows; a frame with no grid is
/// measured by its rectangle; any matrix but the identity keeps the whole
/// resolution, which a perspective may need anywhere. A pane presented at its
/// own size is captured whole, as it always was.
/// `tests::a_pane_drawn_at_a_quarter_is_captured_at_a_quarter`,
/// `tests::a_grid_that_keeps_one_cell_whole_is_captured_whole`,
/// `tests::a_tilted_pane_is_captured_whole`.
pub(crate) fn capture_fraction(
    frame: &present::Frame,
    grid: Option<&crate::warp::Grid>,
    outer: Size<i32, Logical>,
) -> f64 {
    if !frame.matrix.is_identity() {
        return 1.0;
    }
    let (w, h) = (f64::from(outer.w.max(1)), f64::from(outer.h.max(1)));
    let needed = match grid {
        Some(grid) => grid_magnification(grid, w, h),
        None => (frame.rect.size.w / w).max(frame.rect.size.h / h),
    };
    CAPTURE_STEPS
        .iter()
        .rev()
        .copied()
        .find(|step| *step >= needed)
        .unwrap_or(1.0)
}

/// The most `grid` stretches one of its cells' edges on screen, against what
/// that edge spans of a `w` x `h` pane. 1.0 for a grid that does not hold the
/// points its size says, which nothing should make.
fn grid_magnification(grid: &crate::warp::Grid, w: f64, h: f64) -> f64 {
    let (cols, rows) = (grid.cols as usize, grid.rows as usize);
    if cols == 0 || rows == 0 || grid.points.len() != 2 * (cols + 1) * (rows + 1) {
        return 1.0;
    }
    let at = |col: usize, row: usize| {
        let index = 2 * (row * (cols + 1) + col);
        (grid.points[index], grid.points[index + 1])
    };
    let length = |(ax, ay): (f64, f64), (bx, by): (f64, f64)| (bx - ax).hypot(by - ay);
    let across = (grid.part.u1 - grid.part.u0) * w / cols as f64;
    let down = (grid.part.v1 - grid.part.v0) * h / rows as f64;
    let mut most = 0.0_f64;
    for row in 0..=rows {
        for col in 0..=cols {
            if col < cols && across > 0.0 {
                most = most.max(length(at(col, row), at(col + 1, row)) / across);
            }
            if row < rows && down > 0.0 {
                most = most.max(length(at(col, row), at(col, row + 1)) / down);
            }
        }
    }
    most
}

/// A capture's size at the full resolution it stands for, when it was taken
/// at `fraction` of it: what a present's pixels are given as the capture's
/// size (`sol_size`, `sol_box_px`), so a frag's on-screen arithmetic does not
/// move with the resolution (#307).
#[expect(
    clippy::cast_possible_truncation,
    reason = "a capture's size in pixels is a small integer"
)]
fn whole_size(size: Size<i32, Physical>, fraction: f64) -> Size<i32, Physical> {
    (
        (f64::from(size.w) / fraction).round() as i32,
        (f64::from(size.h) / fraction).round() as i32,
    )
        .into()
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
/// under `on_reload = "flat"` it is undeformed. The grid is over the
/// pane's [`warp_part`], its layers' bleed and all. Asked with no renderer.
/// `tests::a_refused_present_follows_its_failed_policy`,
/// `state::tests::real_client::a_present_deform_is_a_file`,
/// `state::tests::real_client::effects_present_is_the_default_a_deform_overrides`.
pub(crate) fn plan_warp(
    state: &mut Solium,
    pane: crate::pane::PaneId,
    outer: Rectangle<i32, Logical>,
    scale: f64,
    now: std::time::Duration,
) -> Planned {
    let Some((frame, aimed)) = warp_of(state, pane, outer, now) else {
        return Planned::Flat;
    };
    let Some(aimed) = aimed else {
        return Planned::Warp {
            frame,
            aimed: None,
            grid: None,
        };
    };
    // Over the pane grown by its layers' bleed, the part its capture holds
    // (#312), as its popups' grid is over theirs.
    let part = warp_part(state, pane);
    let gridded = present_grid(state, pane, &frame, aimed, (WarpPiece::Pane, part), scale);
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
    /// Refused, its folder missing, or a version that never ran: what its
    /// `failed` says.
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
    // A present under `on_reload = "keep"` begun while its version compiled
    // had nothing to pin then: it pins that version the first pass its id
    // answers (`state::tests::real_client::a_kept_present_begun_while_its_version_compiles_keeps_it`).
    if geometry.on_reload == crate::effect::settings::PresentReload::Keep
        && let Some(running) = state.effects.by_id(geometry.effect)
        && let Some(held) = state.panes.get_mut(pane)
        && held.meshes().pinned.is_none()
    {
        held.meshes_mut().pinned = Some(running);
    }
    let Some(loaded) = source_of(state, pane, geometry) else {
        // A version that never ran follows `failed`; one a reload replaced
        // under `on_reload = "flat"` is drawn undeformed
        // (`state::tests::real_client::a_present_whose_frag_will_not_compile_follows_its_failed`).
        return if state.effects.never_ran(geometry.effect) {
            Gridded::Refused
        } else {
            Gridded::Undeformed
        };
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
/// decided not to do. At `now`, the pass's one instant ([`plan_warps`]).
/// `state::tests::real_client::a_fullscreen_window_with_no_rule_takes_todays_path`.
pub(crate) fn warp_of(
    state: &Solium,
    pane: crate::pane::PaneId,
    outer: Rectangle<i32, Logical>,
    now: std::time::Duration,
) -> Option<(present::Frame, Option<present::Aimed>)> {
    let frame = state.drawn_id_at(pane, outer, now);
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
        .is_some_and(|outer| warp_of(state, pane, outer, state.clock.now()).is_some())
}

/// The pass's warp plans, one per pane on screen and shown, made once in
/// `resolve` before `record_boxes` ([`plan_warps`]), so a warped pane's
/// behind slots are placed on the mesh it is drawn through this pass; the
/// `warps` phase takes each one instead of planning again (Ruling F31,
/// amended 2026-10-14, live under motion and warps).
/// `state::tests::real_client::a_warped_panes_behind_slot_is_drawn_beneath_its_warp`.
#[derive(Debug, Default)]
pub(crate) struct Presented {
    planned: std::collections::HashMap<crate::pane::PaneId, Planned>,
    /// The pass's one instant, read once at the top of [`plan_warps`]: what
    /// the warps, the T2 places and the walk are all measured at (amended
    /// 2026-10-14, live under motion and warps).
    /// `state::tests::real_client::a_gliding_windows_t2_place_is_measured_at_the_passs_instant`.
    at: Option<std::time::Duration>,
}

impl Presented {
    /// `state::tests::real_client::a_warped_panes_behind_slot_is_drawn_beneath_its_warp`.
    pub(crate) fn get(&self, pane: crate::pane::PaneId) -> Option<&Planned> {
        self.planned.get(&pane)
    }

    /// The plan the `warps` phase draws: taken, made once a pass.
    pub(crate) fn take(&mut self, pane: crate::pane::PaneId) -> Option<Planned> {
        self.planned.remove(&pane)
    }

    /// `state::tests::real_client::a_gliding_windows_t2_place_is_measured_at_the_passs_instant`.
    pub(crate) fn at(&self) -> Option<std::time::Duration> {
        self.at
    }
}

/// The instant this pass is measured at: the one [`plan_warps`] read, or
/// the clock for a caller before any pass (a test calling a phase alone).
pub(crate) fn pass_now(state: &Solium) -> std::time::Duration {
    state.presented.at().unwrap_or_else(|| state.clock.now())
}

/// The instant the walk draws at: the pass's, when `prepare` made the
/// `Prepared` it is handed, else the clock.
/// `tests::the_walk_draws_at_the_passs_instant`.
pub(crate) fn walk_instant(
    pass: Option<std::time::Duration>,
    clock: std::time::Duration,
) -> std::time::Duration {
    pass.unwrap_or(clock)
}

/// What a pane's warp capture is drawn from: its client, or, for a window
/// whose application has not arrived or is not ready to be seen, the scene
/// standing in for it (#302).
/// `state::tests::real_client::a_loading_pane_under_a_present_is_planned_through_the_warp`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Captured {
    /// The client, frame and all ([`flat_window_elements`]).
    Client(Window),
    /// The standing-in scene in the client's place, with the style's layers
    /// when `loading.decorated` draws them ([`standing_elements`]), as the
    /// walk draws a pane that is `ours`.
    Standing,
}

/// What `pane` is captured from for a warp, and the outer rectangle it is
/// captured over: [`Captured::Standing`] over the pane's own rectangle for a
/// pane that is ours and has a scene, as the walk decides it (no client, or
/// one not ready); else its client over the client's outer rectangle; none
/// for a pane with neither, which nothing draws.
/// `state::tests::real_client::a_loading_pane_under_a_present_is_planned_through_the_warp`.
pub(crate) fn captured(
    state: &Solium,
    pane: crate::pane::PaneId,
    window: Option<&Window>,
) -> Option<(Captured, Rectangle<i32, Logical>)> {
    let ready = window.is_some_and(|window| state.client_ready(window));
    if !ready && state.pane_has_scene(pane) {
        return Some((Captured::Standing, state.pane_outer_of(pane)?));
    }
    let window = window?;
    Some((
        Captured::Client(window.clone()),
        state.outer_geometry(window)?,
    ))
}

/// Plan every warp of the pass, as `warps` planned them: the same panes
/// (on screen, with what to capture and an outer rectangle, [`captured`],
/// `shown`), the same
/// rectangle and scale, the same `plan_warp`, once each, all at the one
/// instant read here. A refusal is still said once and counted once a pass
/// (`state::tests::real_client::a_present_refused_for_its_popups_alone_is_said_once`).
/// `state::tests::real_client::a_warped_panes_behind_slot_is_drawn_beneath_its_warp`,
/// `state::tests::real_client::a_gliding_windows_t2_place_is_measured_at_the_passs_instant`.
pub(crate) fn plan_warps(state: &mut Solium) {
    let now = state.clock.now();
    let mut planned = std::collections::HashMap::new();
    for (pane, window) in state.on_screen() {
        let Some((_, outer)) = captured(state, pane, window.as_ref()) else {
            continue;
        };
        if !shown(state, pane) {
            continue;
        }
        let scale = state.scale_of(outer);
        planned.insert(pane, plan_warp(state, pane, outer, scale, now));
    }
    state.presented = Presented {
        planned,
        at: Some(now),
    };
}

impl Prepared {
    /// Lend the warp captured for `pane`: the texture to draw through it,
    /// the program, the id and commit it carries, and its grid.
    ///
    /// Lent rather than taken: with more than one monitor `elements` runs once
    /// per output, and a texture removed by the first one would leave a
    /// deformed window undrawn on every other screen. `GlesTexture` is a
    /// handle, so the clone is a refcount, and the program is GL names.
    fn warp(&self, pane: crate::pane::PaneId) -> Option<&Warped> {
        self.warps.iter().find(|each| each.pane == pane)
    }

    /// Lend the capture of `pane`'s popups, for the warp drawn in front of
    /// its own, with the part of the pane's unit square it covers. Lent for
    /// `warp`'s reason. `None` when it has no popups open.
    /// `dev/present-check.sh`'s `menu` case draws it.
    fn over(&self, pane: crate::pane::PaneId) -> Option<&Warped> {
        self.overs.iter().find(|each| each.pane == pane)
    }

    /// Whether nothing of `pane` is drawn this pass: a refused present
    /// geometry under `failed = "hide"`.
    /// `tests::a_refused_present_follows_its_failed_policy`.
    fn hidden(&self, pane: crate::pane::PaneId) -> bool {
        self.hidden.contains(&pane)
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
/// Note on the clock: the pass reads it **once**, at the top of `resolve`
/// ([`plan_warps`]), and everything of the pass is measured at that instant:
/// the warps' plans, the T2 places ([`measured`]) and the walk, which
/// `elements` makes at [`Prepared::now`] however much later it runs
/// (amended 2026-10-14, live under motion and warps). A T2 place measured
/// at one instant and walked at another would leave a gliding window's
/// backdrop behind it by its speed times `prepare`'s own time.
/// `state::tests::real_client::a_gliding_windows_t2_place_is_measured_at_the_passs_instant`,
/// `tests::the_walk_draws_at_the_passs_instant`.
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
        frame_tick(state);
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
    let Gpu {
        state,
        warps,
        overs,
        hidden,
        ..
    } = gpu;
    // Once a pass, after the sweep, whatever the number of monitors: the
    // effects compiled, the scenes rendered and the cuts built above, so
    // what the gate waits for is this pass's answer.
    let held = hold_first_frame(state);
    Prepared {
        warps,
        overs,
        hidden,
        slots,
        held,
        now: state.presented.at(),
    }
}

/// Qt's share of a frame, ahead of anything drawn in it, and measured as
/// Qt's.
///
/// The models hosted scenes read, one batch each, ahead of the tick, because
/// applying a row runs every binding and handler on it
/// (`models::tests::publish_models_carries_the_compositors_monitors_to_their_scenes`,
/// `qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`).
/// Then the instant the frame is built at, read once, after them: an exit of
/// Solium's own lock that waits for its first frame is drawn from it
/// (`lock_scene::LockScenes::exit_frame`), and every animation is advanced to
/// it, so the scene's own exit animation, which the publish above started,
/// begins at it too: Qt starts an animation where its clock is when the
/// events that start it are delivered, and the tick delivers them there, or
/// at the next frame's tick inside its 16 ms throttle
/// (`CompositorAnimationDriver` and `solium_qml_tick` in `qml/host.cpp`).
/// However long the publish took, the exit's bound and its animation start
/// together.
/// `lock_auth::tests::the_exit_lasts_exit_ms_from_its_first_frame_however_late_that_comes`.
pub(crate) fn frame_tick(state: &mut Solium) {
    state.publish_models();
    let now = state.clock.now();
    if state.lock_scenes.exit_frame(now) {
        tracing::debug!("the lock's exit is drawn from this frame");
    }
    crate::qml::tick(now);
}

/// The first desktop frame's gate (`warm::Warm`, spec C14), asked once a
/// pass at the end of `prepare`: `true` while [`elements`] draws only the
/// clear colour. A held pass asks for the next one, since a frame of the
/// clear colour damaged nothing and nothing else may come along to draw
/// the frame that releases it; what is still not ready when `max_ms` lets
/// the frame through is named in the log (`warm::Warm::capped`).
/// `state::tests::real_client::a_held_first_frame_asks_for_the_next_pass`,
/// `warm::tests::the_hold_is_capped_at_max_ms`.
pub(crate) fn hold_first_frame(state: &mut Solium) -> bool {
    let ready = state.first_frame_ready();
    let released = state.warm.released();
    let held = state.warm.holding(state.clock.now(), ready);
    if held {
        state.redraw = true;
    } else if !released && state.warm.capped() {
        tracing::warn!(
            compiling = !state.effects.ready(),
            xray = !state.xrays.ready(),
            "warm-up: the first frame drawn at startup.first_frame.max_ms, the effects still not ready"
        );
    }
    held
}

/// `prepare`'s effect phases, so their order is one function a test drives
/// (`tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`).
pub(crate) trait Phases {
    /// Compile what the host was asked for, probe the formats, compile
    /// `Programs::masked` when an effect is wanted (Ruling 7, spec C14).
    fn compile(&mut self);
    /// Plan the pass's warps at its one instant ([`plan_warps`]), resolve
    /// the slots once this pass ([`build_slots`] over the panes a monitor
    /// shows), record their boxes and T2 places ([`record_boxes`], on the
    /// warps' meshes), and want each T2 place's cut and memos ([`want_t2`]).
    fn resolve(&mut self) -> Slots;
    /// Render every hosted scene instance this pass needs as pixels before
    /// `elements`, at the size and scale `elements` will ask for, so
    /// `elements` draws the cached picture ([`scenes_needed`], Ruling F31).
    fn scenes(&mut self, slots: &Slots);
    /// Build each wanted xray cut, a capture kept unless what it is drawn
    /// from differs, then run each wanted memo prefix over a cut redrawn
    /// since it last ran ([`memos_to_run`], [`run_xray`]; Ruling F31).
    fn xray(&mut self, slots: &Slots);
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
/// whole pane's self capture only once the chains inside it have. The
/// scenes a cut holds and the cuts and memos themselves come after
/// `resolve` and before any capture or chain (Ruling F31), since a T2
/// suffix in the inner nest reads them.
/// `tests::prepare_compiles_first_and_builds_the_warps_after_the_chains`,
/// `tests::prepare_renders_scenes_then_xray_before_the_chains`.
pub(crate) fn effect_phases(phases: &mut impl Phases) -> Slots {
    phases.compile();
    let mut slots = phases.resolve();
    phases.scenes(&slots);
    phases.xray(&slots);
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
    hidden: Vec<crate::pane::PaneId>,
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
        // And what a slot crossfading between two rules is drawn through
        // (`effect::crossfade`).
        let _ = state.programs.crossfade(renderer);
        // And what a warped pane and a slot beneath its warp are drawn with,
        // so a slot is never placed beneath a warp that cannot be drawn
        // (`measured` reads their refusals; Ruling F37).
        let _ = state.programs.warp(renderer);
        let _ = state.programs.beneath(renderer);
    }

    fn resolve(&mut self) -> Slots {
        let state = &mut *self.state;
        // The pass's warps, planned once at its one instant, first: a warped
        // pane's behind slot is placed on the mesh it is drawn through this
        // pass (Ruling F31, amended 2026-10-14). With no effects it is the
        // plan `warps` made before, made earlier.
        // `state::tests::real_client::a_warped_panes_behind_slot_is_drawn_beneath_its_warp`.
        plan_warps(state);
        // Every rule resolved once, after the effects compiled and before any
        // job is built; nothing, and no fact gathered, with no rules (spec §8.4,
        // `state::tests::real_client::with_no_rules_no_slot_is_wanted_and_no_fact_is_gathered`).
        let mut slots = build_slots(state);
        // A slot crossfading between two rules keeps the result of the one it
        // leaves, in its `Leaving` twin, rather than start that afresh
        // (`effect::store::tests::a_crossfading_slot_keeps_both_rules_states`).
        for (owner, slot, to, from) in slots.crossfades() {
            state.store.arrange(owner, slot, to, from);
        }
        // Every wanted slot's box, and its state in the store, kept while it is
        // wanted and given back by the sweep once it is not
        // (`effect::store::tests::a_slot_no_rule_wanted_this_pass_is_dropped`).
        record_boxes(state, &mut slots);
        self.pass = state.store.next_pass();
        for (owner, slot, key) in slots.wants() {
            state.store.slot_mut(owner, slot, key).seen = self.pass;
        }
        // Each T2 place's cut and memos wanted, the rest not, so a cut or a
        // memo nobody reads this pass is neither built nor run
        // (`state::tests::real_client::xray_is_wanted_only_on_a_monitor_a_t2_slot_reads`).
        want_t2(state, &mut slots, self.pass);
        slots
    }

    fn scenes(&mut self, _slots: &Slots) {
        let (state, renderer) = (&mut *self.state, &mut *self.renderer);
        // Nothing with no cut wanted, which is every pass with no T2 slot
        // (spec §8.4): `tests::the_scenes_phase_renders_only_what_a_wanted_cut_holds`.
        let needed = scenes_needed(state);
        if needed.is_empty() {
            return;
        }
        let now = state.clock.now();
        for scene in needed {
            let SceneRef::Surface(id, monitor) = scene;
            let Some(output) = output_named(state, &monitor) else {
                continue;
            };
            let (Some(screen), Some((area, alpha))) = (
                state.space.output_geometry(&output),
                surface_drawn_at(state, id, &output),
            ) else {
                continue;
            };
            // As `elements` will draw it: through the same call, at the
            // output's own scale, so its `Gpu::sample` is a hit there, and
            // the xray's, which asks for the same picture (`surface::Raster`).
            let scale = output.current_scale().fractional_scale();
            let _ = scripted(
                state,
                renderer,
                &output,
                id,
                area,
                alpha,
                screen,
                now,
                crate::surface::Raster::at(scale),
            );
        }
    }

    fn xray(&mut self, slots: &Slots) {
        let (state, renderer) = (&mut *self.state, &mut *self.renderer);
        let wanted = state.xrays.wanted();
        // No cut wanted, no capture, no carrier (spec §8.4).
        if wanted.is_empty() {
            return;
        }
        let mut jobs = Vec::new();
        for (monitor, cut) in wanted {
            let Some(output) = output_named(state, &monitor) else {
                continue;
            };
            let Some(job) = crate::offscreen::xray_job(
                state,
                renderer,
                &output,
                cut,
                &crate::effect::xray::SessionLayers,
            ) else {
                continue;
            };
            // Kept while what it is drawn from is the same ([fx0] Ruling 9),
            // never with `SOLIUM_XRAY_REBUILD=always`, the control of every
            // count here (`dev::tests::xray_rebuild_always_is_asked_for_by_name`).
            if !crate::dev::xray_rebuild_always() && crate::offscreen::kept(state, &job).is_some() {
                state.xrays.drawn(&monitor, cut, false);
            } else {
                jobs.push((job, (monitor, cut)));
            }
        }
        for ((monitor, cut), _texture, _id, _commit) in
            crate::offscreen::draw(state, renderer, jobs)
        {
            state.xrays.drawn(&monitor, cut, true);
            crate::pacing::xray_built();
        }
        // Each memo a place reads, run once over its cut when that was
        // redrawn since it last ran
        // (`tests::a_memo_runs_when_its_xray_was_redrawn_and_once_for_every_slot`).
        let run = memos_to_run(&state.memos, &state.xrays, slots.memos());
        if run.is_empty() {
            return;
        }
        let pass = self.pass;
        let Solium {
            store,
            pool,
            effects,
            chains,
            programs,
            timer,
            clock,
            memos,
            xrays,
            ..
        } = &mut *state;
        let mut cx = RunCx {
            store,
            effects: &*effects,
            chains,
            masked: programs.masked_compiled().cloned(),
            now: clock.now().as_secs_f32(),
            pass,
            memos,
            xrays: &*xrays,
            motion: (std::time::Duration::ZERO, 0.0),
            waited: false,
        };
        let _ = with_carrier(pool, renderer, |renderer, carrier, pool| {
            let mut runner = GlRunner::new(renderer, carrier, timer.as_mut());
            run_xray(&mut cx, pool, &mut runner, &run);
        });
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
        let instant = pass_now(state);
        let motion_hz = state.effect_settings.motion_hz;
        let Solium {
            store,
            pool,
            effects,
            chains,
            programs,
            timer,
            clock,
            memos,
            xrays,
            ..
        } = &mut **state;
        let mut cx = RunCx {
            store,
            effects: &*effects,
            chains,
            masked: programs.masked_compiled().cloned(),
            now: clock.now().as_secs_f32(),
            pass: *pass,
            memos,
            xrays: &*xrays,
            motion: (instant, motion_hz),
            waited: false,
        };
        let drawn = &*drawn;
        let _ = with_carrier(pool, renderer, |renderer, carrier, pool| {
            let mut runner = GlRunner::new(renderer, carrier, timer.as_mut());
            run_slots(&mut cx, pool, &mut runner, slots, drawn, nest);
        });
        // A suffix that waited for its next tick is run on a later pass even
        // if nothing else on screen asks for one:
        // `effect::store::tests::a_moving_parts_suffix_runs_at_most_motion_hz_times_a_second`.
        if cx.waited {
            state.redraw = true;
        }
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
            // A window still loading is captured as one that has arrived,
            // its scene in its client's place (#302).
            let Some((captured, outer)) = captured(state, pane, window.as_ref()) else {
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
            // `resolve`'s plan, made at the pass's instant; planned here only
            // for a caller with no `resolve` before it.
            let now = pass_now(state);
            let planned = match state.presented.take(pane) {
                Some(planned) => planned,
                None => plan_warp(state, pane, outer, scale, now),
            };
            let (warp, grid) = match planned {
                Planned::Hidden => {
                    self.hidden.push(pane);
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
                // At the resolution its present draws it at, stepped: a window
                // flying into its icon is not captured at its full size every
                // time its fade or its commits make the capture stale (#307).
                let fraction = capture_fraction(&frame, grid.as_ref(), outer.size);
                let captured_at = scale * fraction;
                // The pane grown by its layers' bleed, which its capture
                // holds, its grid is over and its warp is drawn over (#312).
                let part = warp_part(state, pane);
                // Its client, or the scene standing in for it, at the frame's
                // `client_opacity` inside the capture, so a fading client is
                // captured fading (#296); a window still loading is captured
                // from its scene and layers (#302).
                let job = match &captured {
                    Captured::Client(window) => crate::offscreen::pane_job(
                        state,
                        renderer,
                        pane,
                        window,
                        captured_at,
                        slots,
                        frame.client_opacity,
                    ),
                    Captured::Standing => crate::offscreen::standing_job(
                        state,
                        renderer,
                        pane,
                        captured_at,
                        frame.client_opacity,
                    ),
                };
                // Its present geometry's pixels over its part, once they
                // compiled: the warp draws its capture through them, told
                // the size the capture stands for and where the pane is in it.
                let pixels = aimed.zip(job.as_ref()).and_then(|(aimed, job)| {
                    present_pixels(
                        state,
                        pane,
                        aimed.effect,
                        (part, content_in(part)),
                        whole_size(job.size, fraction),
                    )
                });
                // What its mesh and its pixels are drawn from, in global
                // space: the warp's commit moves when this does, as well as
                // when its capture is redrawn.
                // `keyed::tests::a_warp_whose_mesh_moves_inside_the_same_bounds_is_given_a_new_commit`,
                // `keyed::tests::a_pixels_only_warp_moves_its_commit_with_progress`.
                let shaped = |pixels: &Option<crate::warp::Pixels>| {
                    let shape = Shape::of(&frame, aimed, scale);
                    match pixels {
                        Some((_, uniforms, _)) => shape.with_pixels(*uniforms),
                        None => shape,
                    }
                };
                let shape = shaped(&pixels);
                // Its popups the same way, in a capture of their own that `panes`
                // draws in front of the pane's warp
                // (`tests::a_warped_panes_popups_are_in_front_of_it`), kept until
                // they commit:
                // `state::tests::real_client::a_commit_on_a_popup_makes_the_popups_capture_stale`.
                // Under a present geometry their part has a grid of its own,
                // and a grid refused follows the geometry's `failed`: the
                // popups undeformed, or none of them, the pane's warp kept.
                let over = match (&captured, job.is_some()) {
                    (Captured::Client(window), true) => {
                        crate::offscreen::over_job(state, renderer, pane, window, captured_at)
                    }
                    _ => None,
                };
                let over = match (over, aimed) {
                    (Some((job, part)), Some(aimed)) => {
                        over_grid(state, pane, &frame, aimed, part, scale).map(|grid| {
                            // Their capture whole is their subject.
                            let pixels = present_pixels(
                                state,
                                pane,
                                aimed.effect,
                                (part, [0.0, 0.0, 1.0, 1.0]),
                                whole_size(job.size, fraction),
                            );
                            (job, part, grid, pixels)
                        })
                    }
                    (over, _) => over.map(|(job, part)| (job, part, None, None)),
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
                    // The slots its capture holes, drawn beneath its warp
                    // (Ruling F37), read before the job is drawn or kept.
                    let holes = holes_in(&job.elements);
                    // Already drawn from exactly this: no frame (`offscreen::kept`,
                    // `state::tests::real_client::a_capture_whose_surface_tree_has_not_committed_is_not_drawn_again`).
                    if let Some((texture, id, _commit)) = crate::offscreen::kept(state, &job) {
                        let commit =
                            warp_commit(state, pane, crate::keyed::Kind::Pane, shape, false);
                        let under = unders(state, slots, &holes, scale, commit);
                        self.warps.push(Warped {
                            pane,
                            texture,
                            program,
                            id,
                            commit,
                            part,
                            grid,
                            under,
                            pixels,
                        });
                    } else {
                        jobs.push((
                            job,
                            Then::Warp(program, pane, shape, part, grid, (holes, scale), pixels),
                        ));
                    }
                }
                if let Some((job, part, grid, pixels)) = over {
                    let shape = shaped(&pixels);
                    if let Some((texture, id, _commit)) = crate::offscreen::kept(state, &job) {
                        let commit =
                            warp_commit(state, pane, crate::keyed::Kind::Over, shape, false);
                        self.overs.push(Warped {
                            pane,
                            texture,
                            program,
                            id,
                            commit,
                            part,
                            grid,
                            under: Vec::new(),
                            pixels,
                        });
                    } else {
                        jobs.push((job, Then::Over(program, pane, shape, part, grid, pixels)));
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
                Then::Warp(program, pane, shape, part, grid, (holes, scale), pixels) => {
                    let commit = warp_commit(state, pane, crate::keyed::Kind::Pane, shape, true);
                    let under = unders(state, slots, &holes, scale, commit);
                    self.warps.push(Warped {
                        pane,
                        texture,
                        program,
                        id,
                        commit,
                        part,
                        grid,
                        under,
                        pixels,
                    });
                }
                Then::Over(program, pane, shape, part, grid, pixels) => {
                    let commit = warp_commit(state, pane, crate::keyed::Kind::Over, shape, true);
                    self.overs.push(Warped {
                        pane,
                        texture,
                        program,
                        id,
                        commit,
                        part,
                        grid,
                        under: Vec::new(),
                        pixels,
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
        // A memo no place read for `effects.memo.keep_passes` passes, and a
        // cut nobody wanted for `effects.xray.keep_ms`, give their targets
        // back (Rulings F29, F30): `effect::memo::tests::a_memo_is_kept_for_keep_passes`,
        // `effect::xray::tests::a_cut_nobody_wants_is_kept_for_keep_ms`.
        state.memos.sweep(
            self.pass,
            state.effect_settings.memo.keep_passes,
            &mut state.pool,
        );
        state.xrays.sweep(
            state.clock.now(),
            std::time::Duration::from_millis(state.effect_settings.xray.keep_ms),
            &mut state.pool,
        );
        state.pool.sweep(self.renderer);
    }
}

/// Whether any monitor shows a pane, asked of the rectangle `elements` asks
/// it of: `prepare`'s cull, which [`build_slots`] makes too. A minimized
/// window put away is shown on none, so it is neither captured nor given a
/// slot, and a capture it held is handed back (#30, `Solium::minimized_away`). `prepare`'s walk
/// says at length why the slot and not the client's rectangle.
/// `state::tests::real_client::a_pane_no_monitor_shows_gathers_no_fact_and_wants_no_slot`.
fn shown(state: &Solium, pane: crate::pane::PaneId) -> bool {
    state
        .pane_outer_of(pane)
        .is_some_and(|slot| state.on_any_output(slot))
        && !state
            .panes
            .get(pane)
            .is_some_and(|held| state.minimized_away(held, state.clock.now()))
}

/// What a capture becomes once drawn: `dev/fence-check.sh` has two warps,
/// `dev/present-check.sh`'s menu case a warp and its popups. A warp
/// carries its pane, the [`Shape`] its commit is moved by and the part of
/// the pane's unit square its capture covers (the pane and its layers'
/// bleed, or its popups' rectangle); each the pixels it is drawn through,
/// if any.
#[derive(Debug)]
enum Then {
    /// The pane's own warp over its part, its grid, and the slots its
    /// capture holes with the capture's scale (Ruling F37).
    Warp(
        crate::warp::Program,
        crate::pane::PaneId,
        Shape,
        crate::warp::UnitRect,
        Option<crate::warp::Grid>,
        (Vec<(crate::effect::plan::Owner, Slot)>, f64),
        Option<crate::warp::Pixels>,
    ),
    /// Its popups', over their part, and its grid.
    Over(
        crate::warp::Program,
        crate::pane::PaneId,
        Shape,
        crate::warp::UnitRect,
        Option<crate::warp::Grid>,
        Option<crate::warp::Pixels>,
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

/// The slots a pane's capture holes, topmost first: each `Hole` among the
/// elements its job is drawn from (Ruling F37).
fn holes_in(elements: &[Element]) -> Vec<(crate::effect::plan::Owner, Slot)> {
    elements
        .iter()
        .filter_map(|element| match element {
            Element::Hole(hole) => {
                let (owner, slot) = hole.owner();
                Some((owner.clone(), slot))
            }
            _ => None,
        })
        .collect()
}

/// A warped pane's beneath slots on every monitor, in its capture's order:
/// each hole's slot with its T2 texture there (`Slots::screened`), its
/// shape in the capture's pixels at `scale`, the capture the pane grown by
/// its layers' bleed (`tests::a_hole_is_masked_at_its_place_in_a_capture_that_bleeds`),
/// the T2 result's per-monitor
/// id, and a commit that moves with the warp's or a re-run
/// (`effect::store::tests::an_unders_commit_moves_with_its_warp_or_its_rerun`).
/// What `dev/effects-check.sh motion`'s `genie` draws beneath its funnel.
fn unders(
    state: &mut Solium,
    slots: &Slots,
    holes: &[(crate::effect::plan::Owner, Slot)],
    scale: f64,
    warp: CommitCounter,
) -> Vec<crate::effect::beneath::Under> {
    let mut unders = Vec::new();
    for (owner, hole) in holes {
        // A slot crossfading between two rules has both its results beneath
        // the warp, each at its share of the slot's opacity, laid one over
        // the other there rather than mixed in one draw as the flat path
        // mixes them (`effect::crossfade`): a dip half way through, for as
        // long as a crossfade and a warp meet.
        let whole = slots.opacity(owner, *hole);
        let legs = match slots.mix(owner, *hole) {
            Some(share) => [
                Some((*hole, whole * share)),
                Some((hole.leaving(), whole * (1.0 - share))),
            ],
            None => [Some((*hole, whole)), None],
        };
        for (slot, opacity) in legs.into_iter().flatten() {
            under(
                state,
                slots,
                owner,
                slot,
                opacity,
                (scale, warp),
                &mut unders,
            );
        }
    }
    unders
}

/// One result beneath a warped pane on every monitor it is placed on, at
/// `opacity`, pushed onto `into`: [`unders`]' body, once per result.
fn under(
    state: &mut Solium,
    slots: &Slots,
    owner: &crate::effect::plan::Owner,
    slot: Slot,
    opacity: f32,
    (scale, warp): (f64, CommitCounter),
    into: &mut Vec<crate::effect::beneath::Under>,
) {
    let crate::effect::plan::Owner::Pane(pane, _) = owner else {
        return;
    };
    let (Some(outer), Some((found, _)), Some(key), Some(boxed)) = (
        state.pane_outer_of(*pane),
        part_of(state, owner),
        slots.key(owner, slot),
        slots.boxed(owner, slot),
    ) else {
        return;
    };
    let Some(rest) = part_rect(state, &found) else {
        return;
    };
    // In the capture, which holds the pane's layers' bleed around it
    // (#312): the part's place in the pane grown by it.
    let captured = bled(outer, capture_bleed(state, *pane));
    let mask = crate::effect::beneath::BeneathMask::Shape(crate::effect::beneath::beneath_mask(
        rest,
        captured,
        boxed.radii,
        scale,
    ));
    for place in slots.places(owner, slot) {
        let Some(screened) = slots.screened(owner, slot, &place.monitor) else {
            continue;
        };
        let t2 = state
            .store
            .slot_mut(owner, slot, key)
            .t2_mut(&place.monitor);
        into.push(crate::effect::beneath::Under {
            monitor: place.monitor.clone(),
            texture: screened.texture.clone(),
            over: screened.over,
            mask: mask.clone(),
            id: t2.id.clone(),
            commit: t2.under_commit(warp, screened.rerun),
            opacity,
        });
    }
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
    use crate::effect::plan::{Owner, Slots};
    if state.rules.is_empty()
        && state
            .panes
            .iter()
            .all(|pane| style_rules(pane.frame()).0.is_empty())
    {
        // No rule is left to fade a slot either.
        state.slot_fades.clear();
        return Slots::default();
    }
    // While the session is locked no slot of the session is wanted, T1 or
    // T2: nothing of it is captured (a self or a mask capture), run or read
    // behind the lock, and no xray is built ([16] §5, Ruling F29). The
    // pointer's own scene keeps its slots from Task 61 on.
    // `state::tests::real_client::nothing_of_the_session_is_captured_while_locked`.
    //
    // But for the entry of Solium's own lock, while the desktop is still on
    // screen under its scenes: drawn, it is drawn with its effects, so it
    // does not change under the sheet coming down. A capture of the screen
    // is still refused (`screencopy.rs`).
    if state.lock.is_some() && !state.desktop_under_lock_anywhere(pass_now(state)) {
        return Slots::default();
    }
    // At the pass's instant, as the walk draws: a slot fading in or out with
    // its rule (`effect::fade`) is shown at its opacity then.
    let now = pass_now(state);
    let mut fades = std::mem::take(&mut state.slot_fades);
    let slots = resolve_slots(state, &mut fades, now);
    // A slot whose pane is out of sight this pass keeps its fade; one whose
    // owner has gone does not.
    fades.sweep(|owner| match owner {
        Owner::Pane(pane, _) => state.panes.get(*pane).is_some(),
        Owner::Surface(..) | Owner::LayerShell(_) => false,
    });
    // A fade draws no new picture and damages nothing, so its frames are
    // asked for here or it stops where it stands.
    // `state::tests::real_client::a_rule_with_a_fade_eases_its_slot_in_and_out_with_the_focus`.
    if fades.moving(now) {
        state.redraw = true;
    }
    state.slot_fades = fades;
    slots
}

/// [`build_slots`]' walk, its slots stepped through `fades` at `now`.
fn resolve_slots(
    state: &Solium,
    fades: &mut crate::effect::fade::Fades,
    now: std::time::Duration,
) -> crate::effect::plan::Slots {
    use crate::effect::plan::{self, PaneSlot, Slots};
    use crate::effect::rules::{Facts, PartRef};
    use smithay::reexports::wayland_server::Resource as _;
    let mut slots = Slots::default();
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
                (&mut *fades, now),
                &plan::Owner::Pane(pane, slot_of),
                (style, state.rules.resolve(style, generation, part, &view)),
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
                want_resolved(
                    &mut slots,
                    state,
                    (&mut *fades, now),
                    &plan::Owner::Surface(surface.id(), output.name()),
                    (&[], resolved),
                );
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
            want_resolved(
                &mut slots,
                state,
                (&mut *fades, now),
                &plan::Owner::LayerShell(layer.wl_surface().id()),
                (&[], resolved),
            );
        }
    }
    slots
}

/// Want each slot a part resolved to whose chain is bound, stepped through
/// `fades` at `now`: a rule with a `fade` is wanted at the opacity its fade
/// has reached, and still wanted, fading out, once it stops matching, until
/// it is gone (`effect::fade`); one without is wanted whole while it matches.
/// A slot crossfading from another rule wants that rule in its `Leaving`
/// twin too, with how far the crossfade has got; a `replace` whose rule
/// fades is mixed with its part's own pixels instead of fading to nothing.
/// `style` is the pane's style's rules, which a key of the style's names.
/// `state::tests::real_client::a_rule_whose_chain_is_not_bound_wants_no_slot`,
/// `state::tests::real_client::a_rule_with_a_fade_eases_its_slot_in_and_out_with_the_focus`,
/// `state::tests::real_client::two_replace_rules_on_focused_crossfade_as_the_keyboard_moves`,
/// `state::tests::real_client::a_replace_rule_coming_and_going_crossfades_with_its_part`.
fn want_resolved(
    slots: &mut crate::effect::plan::Slots,
    state: &Solium,
    (fades, now): (&mut crate::effect::fade::Fades, std::time::Duration),
    owner: &crate::effect::plan::Owner,
    (style, resolved): (
        &[crate::effect::rules::Rule],
        crate::effect::rules::Resolved,
    ),
) {
    use crate::effect::rules::Slot;
    for slot in [Slot::Behind, Slot::Front, Slot::Replace] {
        let bound = resolved
            .get(slot)
            .filter(|key| state.chains.get(*key).is_some());
        let with_fade = bound.map(|key| {
            let fade = state.rules.rule(style, key).and_then(|rule| rule.fade);
            (key, fade)
        });
        let Some(shown) = fades.step(
            owner,
            slot,
            with_fade,
            |key| state.chains.get(key).is_some(),
            now,
        ) else {
            continue;
        };
        let Some(chain) = state.chains.get(shown.key) else {
            continue;
        };
        // A T2 chain on a part no xray layer lies below (a part on
        // `background`) reads nothing: no slot, and no problem (Ruling
        // F29). `effect::xray::tests::a_part_reads_the_xray_layers_strictly_below_its_band`.
        if chain.tier == crate::effect::rules::Tier::Xray && t2_cut(state, owner, chain).is_none() {
            continue;
        }
        slots.want(owner.clone(), slot, shown.key);
        // A `replace` hands its part over to its result and back as its rule
        // comes and goes, mixing the two, so nothing behind the window shows
        // through it on the way and nothing pops at the end; `behind` and
        // `front` only add, and come from and go to nothing.
        if shown.opacity < 1.0 {
            if slot == Slot::Replace {
                slots.set_raw(owner.clone(), slot, 1.0 - shown.opacity);
            } else {
                slots.set_opacity(owner.clone(), slot, shown.opacity);
            }
        }
        // Crossfading from another rule: that rule's result is wanted too, in
        // the slot's `Leaving` twin, so it is kept and run beside this one's
        // and drawn mixed with it (`effect::fade`). One that reads nothing
        // here, as above, leaves this one's alone.
        if let Some((from, share)) = shown.from
            && let Some(leaving) = state.chains.get(from)
            && (leaving.tier != crate::effect::rules::Tier::Xray
                || t2_cut(state, owner, leaving).is_some())
        {
            slots.want(owner.clone(), slot.leaving(), from);
            slots.set_mix(owner.clone(), slot, share);
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
        let Some(part) = part else {
            continue;
        };
        // A T2 slot's places, one per monitor its padded box overlaps
        // (Ruling F30), measured where the part is drawn this pass (Ruling
        // F37: `measured`), each logged as `effect slot` for the nested
        // checks:
        // `state::tests::real_client::a_window_across_two_monitors_has_a_t2_place_on_each`,
        // `state::tests::real_client::a_gliding_windows_t2_place_follows_its_drawn_rectangle`.
        if chain.tier == crate::effect::rules::Tier::Xray
            && let Some(cut) = t2_cut(state, &owner, chain)
        {
            let measured = measured(state, &owner, slot, &found);
            // Drawn beneath its pane's warp, out of the pane's capture:
            // `state::tests::real_client::a_warped_panes_behind_slot_is_drawn_beneath_its_warp`.
            if matches!(measured, Some(Measured::Beneath(_))) {
                slots.set_beneath(owner.clone(), slot);
            }
            let places = measured.map_or_else(Vec::new, |measured| {
                t2_places(state, &owner, &found, (cut, scale, pad), &measured, part)
            });
            for place in &places {
                let at = place.screen;
                tracing::debug!(
                    ?owner,
                    part = part_name(&owner),
                    monitor = %place.monitor,
                    padded = %format!("{},{},{},{}", at.loc.x, at.loc.y, at.size.w, at.size.h),
                    "effect slot"
                );
            }
            slots.set_places(owner.clone(), slot, places);
        }
        slots.set_box(owner, slot, part);
    }
}

/// A slot owner's part as a rule names it, for the `effect slot` log line
/// Task 46's nested frost check reads.
fn part_name(owner: &crate::effect::plan::Owner) -> String {
    use crate::effect::plan::Owner as Whose;
    match owner {
        Whose::Pane(_, PaneSlot::Pane) => "pane".to_owned(),
        Whose::Pane(_, PaneSlot::Client) => "client".to_owned(),
        Whose::Pane(_, PaneSlot::Popups) => "popup".to_owned(),
        Whose::Pane(_, PaneSlot::Titlebar) => "region:titlebar".to_owned(),
        Whose::Pane(_, PaneSlot::Layer(index)) => format!("layer:{index}"),
        Whose::Surface(..) => "surface".to_owned(),
        Whose::LayerShell(_) => "layer_shell".to_owned(),
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
            // And a `replace` mixed with its part as its rule fades, whatever
            // its chain reads: the part's own pixels are what it mixes with
            // (`slot_element`).
            if chain.tier != crate::effect::rules::Tier::Own && slots.raw(owner, slot).is_none() {
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
    /// Every memo: what `run_xray` runs into and a T2 place reads.
    pub(crate) memos: &'a mut crate::effect::memo::Memos,
    /// Every xray cut: what a memo prefix and a T2 run bind as `backdrop`.
    pub(crate) xrays: &'a crate::effect::xray::Xrays,
    /// The pass's instant, which a run is marked with, and
    /// `effects.motion_hz`: how often, at most, a chain moved only by its
    /// part's place or size runs again
    /// ([`crate::effect::store::SlotState::waits`],
    /// [`crate::effect::store::T2State::waits`]).
    pub(crate) motion: (std::time::Duration, f64),
    /// Whether a chain waited for its next motion tick this pass: the pass
    /// that runs it at its part's last place and size is then owed, whether
    /// or not the part moves again.
    pub(crate) waited: bool,
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
        // T2: each place on its own monitor, from that monitor's memo
        // (Ruling F30;
        // `state::tests::real_client::a_window_across_two_monitors_has_a_t2_place_on_each`).
        if chain.tier == Tier::Xray {
            if run_t2(cx, pool, runner, slots, &owner, slot, key, &mut began) {
                let _ = cx.chains.refuse_once(
                    key,
                    "an effect's chain failed; its part is drawn without it",
                );
            }
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
            // T3 was refused at load (Ruling 14); T2 ran above.
            Tier::Xray | Tier::Live => continue,
        };
        let size = part.size();
        let params = chain.params_hash;
        let keys = Keys {
            params,
            own: drawn.commit(&owner, slot),
            shape: shape_hash(part.content, part.radii),
        };
        let (instant, hz) = cx.motion;
        let state = cx.store.slot_mut(&owner, slot, key);
        state.seen = cx.pass;
        // Run again only when its input was redrawn, its params changed or
        // its box changed size; a T0 chain whose box only changed size, run
        // already in this tick of `effects.motion_hz`, draws its last result
        // stretched over the part and runs at the next tick
        // (`effect::store::tests::a_resized_parts_t0_chain_runs_at_most_motion_hz_times_a_second`).
        let motion = crate::effect::store::Motion {
            hz,
            phase: motion_phase(&owner, slot, ""),
        };
        let waits = chain.tier == Tier::Generated
            && state.held.output().is_some()
            && state.waits(redrawn, params, size, instant, motion);
        cx.waited |= waits;
        if waits || !state.needs_run(redrawn, params, size) {
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
            clamp_first: slot.placed() == Slot::Replace && chain.tier == Tier::Own,
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
                state.ran_at = Some(instant);
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

/// One T2 slot's places, each on its own monitor (Ruling F30): a crop of
/// its memo placed with no pass (`T2Work::Crop`), else its suffix or, over
/// `effects.memo.budget_mib`, its whole plan run at the part's own size,
/// reading the cut through the place's `BoxMap`, only when its `T2Key`
/// moved and otherwise its last result placed again. Each place reads its
/// own `part`, the drawn part's box where it moves (Ruling F37). A place
/// whose cut or memo has no picture yet draws nothing this pass. A slot
/// drawn beneath its pane's warp also leaves each place's texture for the
/// warp's `Under`s (`Slots::set_screened`). Whether a run failed.
/// `tests::an_empty_suffix_is_a_crop_and_a_suffix_runs`,
/// `state::tests::real_client::a_t2_slot_with_memo_false_reruns_when_its_xray_is_redrawn`;
/// what it draws is wirecheck 13a and 13b's (Task 45), and beneath a warp
/// 13i's.
#[expect(
    clippy::too_many_arguments,
    reason = "`run_slots`' own context, and the one slot it is on"
)]
fn run_t2(
    cx: &mut RunCx<'_>,
    pool: &mut crate::pool::Pool,
    runner: &mut dyn Runner,
    slots: &mut Slots,
    owner: &crate::effect::plan::Owner,
    slot: Slot,
    rule: crate::effect::rules::RuleKey,
    began: &mut bool,
) -> bool {
    use crate::effect::memo::MemoKey;
    use crate::effect::run::{BoxMap, Inputs, Keys, Outcome};
    use smithay::backend::renderer::Texture as _;
    let Some(chain) = cx.chains.get(rule) else {
        return false;
    };
    let Some(memo) = chain.memo() else {
        return false;
    };
    let places = slots.places(owner, slot).to_vec();
    let (instant, hz) = cx.motion;
    let waited = &mut cx.waited;
    let state = cx.store.slot_mut(owner, slot, rule);
    state.seen = cx.pass;
    // A monitor the part left gives its result's targets back
    // (`effect::store::tests::a_t2_result_on_a_monitor_left_is_given_back`,
    // `state::tests::real_client::a_part_that_left_a_monitor_gives_its_t2_result_there_back`).
    let on: Vec<&str> = places.iter().map(|place| place.monitor.as_str()).collect();
    state.keep_t2(&on, pool);
    let mut failed = false;
    for place in places {
        let Some(xray) = cx.xrays.texture(&place.monitor, place.cut).cloned() else {
            continue;
        };
        let work = t2_work(memo, slots.shared_on(owner, slot, &place.monitor));
        let motion = crate::effect::store::Motion {
            hz,
            phase: motion_phase(owner, slot, &place.monitor),
        };
        let part = place.part;
        let now = t2_key(
            cx.memos,
            cx.xrays,
            &place,
            chain,
            &work,
            part.content,
            part.radii,
            None,
        );
        let memo_of = |upto: u64| MemoKey {
            monitor: place.monitor.clone(),
            cut: place.cut,
            upto,
        };
        let t2 = state.t2_mut(&place.monitor);
        let (texture, rerun, crop) = match &work {
            // No pass: the memo, cropped to the part's padded box, its
            // commit moved when the memo re-ran or the crop moved, and
            // nothing part-sized held for it
            // (`effect::store::tests::a_t2_place_that_turns_to_a_crop_gives_its_targets_back`).
            T2Work::Crop(export) => {
                let Some(upto) = memo
                    .exports
                    .iter()
                    .position(|each| each == export)
                    .and_then(|index| chain.memo_keys().get(index))
                else {
                    continue;
                };
                let Some(texture) = cx.memos.output(&memo_of(*upto)).cloned() else {
                    continue;
                };
                let rerun = t2.cropped(now, pool);
                let size = texture.size();
                let crop = crop_of(
                    place.map,
                    (
                        u32::try_from(size.w).unwrap_or(1),
                        u32::try_from(size.h).unwrap_or(1),
                    ),
                );
                (texture, rerun, Some(crop))
            }
            T2Work::Suffix | T2Work::Whole if !t2.needs_run(now) => {
                let Some(texture) = t2.held.output().cloned() else {
                    continue;
                };
                (texture, false, None)
            }
            // Moved only by its part's place or size, and run already in this
            // tick of `effects.motion_hz`: its last result is drawn
            // where the part is, and it runs again at the next tick
            // (`effect::store::tests::a_moving_parts_suffix_runs_at_most_motion_hz_times_a_second`).
            T2Work::Suffix | T2Work::Whole
                if t2.held.output().is_some() && t2.waits(now, instant, motion) =>
            {
                let Some(texture) = t2.held.output().cloned() else {
                    continue;
                };
                *waited = true;
                (texture, false, None)
            }
            T2Work::Suffix | T2Work::Whole => {
                let plan = if work == T2Work::Whole {
                    &chain.plan
                } else {
                    &memo.suffix
                };
                let mut textures: Vec<(&str, GlesTexture, BoxMap)> =
                    vec![("backdrop", xray, place.map)];
                if work == T2Work::Suffix {
                    let mut missing = false;
                    for (export, upto) in memo.exports.iter().zip(chain.memo_keys()) {
                        match cx.memos.output(&memo_of(*upto)) {
                            Some(texture) => {
                                textures.push((export.name.as_str(), texture.clone(), place.map));
                            }
                            None => missing = true,
                        }
                    }
                    if missing {
                        continue;
                    }
                }
                let inputs = Inputs {
                    padded: (
                        u32::try_from(place.own.w).unwrap_or(1),
                        u32::try_from(place.own.h).unwrap_or(1),
                    ),
                    content: part.content,
                    textures: &textures,
                    radii: part.radii,
                    time: cx.now,
                    transition: crate::effect::run::Transition::default(),
                    clamp_first: false,
                };
                let keys = Keys {
                    params: chain.params_hash,
                    own: None,
                    shape: shape_hash(part.content, part.radii),
                };
                if !*began {
                    runner.begin();
                    *began = true;
                }
                let effects = cx.effects;
                match runner.run(
                    pool,
                    &|program| effects.lookup(program),
                    effects.formats(),
                    plan,
                    &mut t2.held,
                    &inputs,
                    &keys,
                ) {
                    Outcome::Done(texture, sync) => {
                        let _ = crate::offscreen::settle(
                            Ok::<_, std::convert::Infallible>(sync),
                            crate::dev::fence_wait(),
                            "an effect",
                        );
                        crate::pacing::effect_ran();
                        t2.key = Some(now);
                        t2.ran_at = Some(instant);
                        (texture, true, None)
                    }
                    // Not compiled yet, or a format waiting for the rebind:
                    // nothing behind the part this pass, nothing latched.
                    Outcome::Pending => continue,
                    Outcome::Failed => {
                        failed = true;
                        continue;
                    }
                }
            }
        };
        if slots.is_beneath(owner, slot) {
            // Drawn beneath the warp, sampled where each fragment is: a
            // crop's whole memo spans the monitor, a run's result its box
            // (Ruling F37). `tests::a_beneath_slot_is_a_hole_only_inside_a_capture`.
            slots.set_screened(
                owner.clone(),
                slot,
                &place.monitor,
                crate::effect::beneath::Screened {
                    texture: texture.clone(),
                    over: if crop.is_some() {
                        crate::effect::beneath::Over::Monitor
                    } else {
                        crate::effect::beneath::Over::Box(place.screen)
                    },
                    rerun,
                },
            );
        }
        if let Some(ready) = t2_ready(t2, cx.masked.as_ref(), texture, rerun, part, crop) {
            slots.set_ready_on(owner.clone(), slot, &place.monitor, ready);
        }
    }
    failed
}

/// Where in one interval of `effects.motion_hz` a slot's ticks fall, 0 to
/// 1, from its owner, slot and monitor (a T2 place's own; `""` for a T0
/// slot's one result): fixed for the slot, and spread across slots, so the
/// glass and the shade of windows that start moving together run on
/// different passes rather than all on one.
/// `effect::store::tests::a_moving_parts_suffix_runs_at_most_motion_hz_times_a_second`.
fn motion_phase(owner: &crate::effect::plan::Owner, slot: Slot, monitor: &str) -> f64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (owner, slot, monitor).hash(&mut hasher);
    #[expect(
        clippy::cast_precision_loss,
        reason = "a share of 1024, exact in an f64"
    )]
    let share = (hasher.finish() % 1024) as f64 / 1024.0;
    share
}

/// A T2 place's result as the walk places it on its monitor: the texture in
/// an `EffectElement` with that monitor's own id, cropped when it is a
/// memo's, the crop kept by every later `at` (`EffectElement::cropped`),
/// its commit moved only when it re-ran or its placement (a crop included)
/// differs; `None` with no masked program, as [`ready_from`].
/// `effect::store::tests::a_t2_results_commit_moves_only_when_rerun_or_placed_elsewhere`,
/// `effect::element::tests::a_cropped_result_placed_again_keeps_its_crop`.
fn t2_ready(
    t2: &mut crate::effect::store::T2State,
    masked: Option<&GlesTexProgram>,
    texture: GlesTexture,
    rerun: bool,
    part: crate::effect::plan::PartBox,
    crop: Option<Rectangle<f64, smithay::utils::Buffer>>,
) -> Option<crate::effect::plan::Ready> {
    let program = masked?.clone();
    let placement = crate::effect::element::Placement::of(part.padded, None, 1.0);
    let placement = crop.map_or(placement, |crop| placement.cropped(crop));
    let commit = t2.commit_for(placement, rerun);
    let element =
        crate::effect::element::EffectElement::new(t2.id.clone(), commit, texture, program);
    let element = match crop {
        Some(crop) => element.cropped(crop),
        None => element,
    };
    Some(crate::effect::plan::Ready {
        element,
        padded: part.padded,
        reach: part.reach,
    })
}

/// The output of this name, if it is still in the space.
/// `state::tests::real_client::reflow_on_close::stacking::the_scenes_phase_renders_only_what_a_wanted_cut_holds`.
fn output_named(state: &Solium, name: &str) -> Option<smithay::output::Output> {
    state
        .space
        .outputs()
        .find(|output| output.name() == name)
        .cloned()
}

/// One hosted scene instance whose pixels this pass needs before
/// `elements` (Ruling F31). Task 60 adds pane-style decorations and the
/// pointer. `tests::the_scenes_phase_renders_only_what_a_wanted_cut_holds`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SceneRef {
    Surface(crate::scripted::SurfaceId, String),
}

/// Every scripted surface in a wanted xray cut, on that cut's monitor, each
/// once: what the `scenes` phase renders.
/// `state::tests::real_client::reflow_on_close::stacking::the_scenes_phase_renders_only_what_a_wanted_cut_holds`.
pub(crate) fn scenes_needed(state: &Solium) -> Vec<SceneRef> {
    use crate::effect::xray::XraySource as _;
    let mut needed: Vec<SceneRef> = Vec::new();
    for (monitor, cut) in state.xrays.wanted() {
        let Some(output) = output_named(state, &monitor) else {
            continue;
        };
        for each in crate::effect::xray::SessionLayers.entries(state, &output, cut) {
            if let Stacked::Surface(id, ..) = each {
                let scene = SceneRef::Surface(id, monitor.clone());
                if !needed.contains(&scene) {
                    needed.push(scene);
                }
            }
        }
    }
    needed
}

/// Where a scripted surface's instance on `output` is drawn this pass, and
/// at what alpha: its declared area carried by its selection, as
/// [`stacked`] lists it (`wanted`'s two calls). `None` with no instance
/// there. What the `scenes` phase renders it at, so `elements`' sample is a
/// hit: `dev/effects-check.sh xray` finds a moving wallpaper's `rendered`
/// once a pass with a T2 rule, as without one.
fn surface_drawn_at(
    state: &Solium,
    id: crate::scripted::SurfaceId,
    output: &smithay::output::Output,
) -> Option<(Rectangle<i32, Logical>, f32)> {
    let geometry = state.space.output_geometry(output)?;
    let primary = state.primary_output();
    let area = state
        .surfaces
        .get(id)?
        .area_on(output, geometry, primary.as_ref())?;
    Some((
        state.carried(id, output, area),
        state.carried_alpha(id, output),
    ))
}

/// The wanted memos whose xray was redrawn since they ran (or that never
/// ran, or whose run made nothing), each once, in the order wanted; a memo
/// whose cut has no picture yet waits for one.
/// `tests::a_memo_runs_when_its_xray_was_redrawn_and_once_for_every_slot`.
pub(crate) fn memos_to_run(
    memos: &crate::effect::memo::Memos,
    xrays: &crate::effect::xray::Xrays,
    wanted: &[crate::effect::memo::MemoKey],
) -> Vec<crate::effect::memo::MemoKey> {
    let mut run: Vec<crate::effect::memo::MemoKey> = Vec::new();
    for key in wanted {
        let Some(generation) = xrays.generation(&key.monitor, key.cut) else {
            continue;
        };
        if memos.needs_run(key, generation) && !run.contains(key) {
            run.push(key.clone());
        }
    }
    run
}

/// What a T2 place does with its memo: place a crop of it (an empty
/// suffix, no pass), run its suffix, or, when its memos were not held (over
/// `effects.memo.budget_mib`), run its whole plan at the part's size.
/// `tests::an_empty_suffix_is_a_crop_and_a_suffix_runs`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum T2Work {
    /// An empty suffix: a crop of this export's memo, no pass.
    Crop(solium_effects::stage::Export),
    /// The suffix, at the part's own size, over the memos and the xray.
    Suffix,
    /// No memo held (over `effects.memo.budget_mib`): the whole plan at the
    /// part's own size, over the xray.
    Whole,
}

/// `tests::an_empty_suffix_is_a_crop_and_a_suffix_runs`.
pub(crate) fn t2_work(memo: &solium_effects::stage::Memo, shared: bool) -> T2Work {
    if !shared {
        return T2Work::Whole;
    }
    match (memo.suffix.steps.is_empty(), memo.exports.first()) {
        (true, Some(export)) => T2Work::Crop(export.clone()),
        _ => T2Work::Suffix,
    }
}

/// What a T2 place's result is made from (Ruling F52's struct): a hash of
/// the memos it reads, each as its prefix key (`upto`) with its generation,
/// so reading another memo of the same generation moves it too (0 for
/// `Whole`, and for a chain with no memo); the generation of the cut it
/// binds as `backdrop`, which every T2 run reads, so a run that reads no
/// memo still re-runs when the wallpaper or a `bottom` surface redraws; the
/// padded box in the monitor's own physical pixels (`T2Place.screen`, never
/// its xray pixels, so a drag finer than an xray pixel re-runs it, Ruling
/// F37); the params; the shape; and the self capture's commit (Task 52).
/// Pure. `tests::a_t2_key_reads_each_memo_by_its_prefix_and_generation`,
/// `tests::a_t2_key_moves_with_a_drag_finer_than_an_xray_pixel`,
/// `state::tests::real_client::a_drag_finer_than_an_xray_pixel_moves_the_t2_key_at_half_scale`,
/// `state::tests::real_client::a_t2_slot_with_memo_false_reruns_when_its_xray_is_redrawn`,
/// `state::tests::real_client::a_t2_slot_over_the_memo_budget_reruns_when_its_xray_is_redrawn`.
#[expect(
    clippy::too_many_arguments,
    reason = "everything a T2 result is made from, as the plan names it"
)]
pub(crate) fn t2_key(
    memos: &crate::effect::memo::Memos,
    xrays: &crate::effect::xray::Xrays,
    place: &crate::effect::plan::T2Place,
    chain: &crate::effect::plan::BoundChain,
    work: &T2Work,
    content: [f32; 4],
    radii: [f32; 4],
    own: Option<u64>,
) -> crate::effect::memo::T2Key {
    use std::hash::{Hash, Hasher};
    let memo = match work {
        T2Work::Whole => 0,
        T2Work::Crop(_) | T2Work::Suffix if chain.memo_keys().is_empty() => 0,
        T2Work::Crop(_) | T2Work::Suffix => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for upto in chain.memo_keys() {
                let key = crate::effect::memo::MemoKey {
                    monitor: place.monitor.clone(),
                    cut: place.cut,
                    upto: *upto,
                };
                (*upto, memos.generation(&key)).hash(&mut hasher);
            }
            hasher.finish()
        }
    };
    crate::effect::memo::T2Key {
        memo,
        xray: xrays.generation(&place.monitor, place.cut).unwrap_or(0),
        padded: place.screen,
        params: chain.params_hash,
        shape: shape_hash(content, radii),
        own,
    }
}

/// A part's box in a memo's own pixels.
/// `tests::a_crop_is_the_parts_box_in_the_memos_pixels`.
pub(crate) fn crop_of(
    map: crate::effect::run::BoxMap,
    memo: (u32, u32),
) -> Rectangle<f64, smithay::utils::Buffer> {
    let (w, h) = (f64::from(memo.0), f64::from(memo.1));
    Rectangle::new(
        (f64::from(map.offset[0]) * w, f64::from(map.offset[1]) * h).into(),
        (f64::from(map.scale[0]) * w, f64::from(map.scale[1]) * h).into(),
    )
}

/// The cut a T2 chain on `owner` reads: the configured xray layers
/// strictly below its part's band (`effects.xray.layers`), then the rule's
/// `cut`, `background` turning a bottom cut into the wallpaper alone; none
/// for a part on `background`, which then wants no slot.
/// `state::tests::real_client::a_rule_may_read_the_wallpaper_alone`.
fn t2_cut(
    state: &Solium,
    owner: &crate::effect::plan::Owner,
    chain: &crate::effect::plan::BoundChain,
) -> Option<crate::effect::xray::Cut> {
    use crate::effect::plan::Owner as Whose;
    use crate::stack::{Band, Owner as By};
    use smithay::reexports::wayland_server::Resource as _;
    let band = match owner {
        Whose::Pane(..) => Band::Windows,
        Whose::Surface(id, _) => Band::Layer(state.surfaces.get(*id)?.layer(), By::Script),
        Whose::LayerShell(object) => state.space.outputs().find_map(|output| {
            layer_map_for_output(output)
                .layers()
                .find(|layer| layer.wl_surface().id() == *object)
                .map(|layer| Band::Layer(layer::ours(layer.layer()), By::Client))
        })?,
    };
    let cut = state.effect_settings.xray.cut_below(band)?;
    Some(match chain.cut {
        crate::effect::rules::CutChoice::Below => cut,
        crate::effect::rules::CutChoice::Background => crate::effect::xray::Cut::Background,
    })
}

/// Where a part lies at rest, in global logical pixels: the rectangle its
/// T2 places are measured from (Ruling F37: a warped pane's at rest).
/// `state::tests::real_client::a_window_across_two_monitors_has_a_t2_place_on_each`.
fn part_rect(state: &Solium, found: &Found) -> Option<Rectangle<i32, Logical>> {
    let outer_of = |window: &Window| {
        state
            .panes
            .of(window)
            .map(|pane| state.pane_outer(pane))
            .or_else(|| state.outer_geometry(window))
    };
    let client_of = |window: &Window| {
        let outer = outer_of(window)?;
        let client = flat(state, window)?.client;
        Some(Rectangle::new(outer.loc + client.loc, client.size))
    };
    match found {
        Found::Client(window) => client_of(window),
        Found::Popups(window) => {
            let client = client_of(window)?;
            let covered = popups_covered(window)?;
            Some(Rectangle::new(client.loc + covered.loc, covered.size))
        }
        Found::Pane(window) => outer_of(window),
        Found::Layer(pane, index) => {
            let outer = state.pane_outer_of(*pane)?;
            let canvas = layer_canvas(state, *pane, *index)?;
            Some(Rectangle::new(outer.loc + canvas.loc, canvas.size))
        }
        Found::Titlebar(pane) => {
            let held = state.panes.get(*pane)?;
            let outer = state.pane_outer(held);
            let regions = crate::effect::mask::FromInsets {
                insets: held.decoration()?.insets(),
                outer: outer.size,
                radii: declared_rounding(state, *pane)
                    .map_or(Corners::all(0.0), |effect| effect.radii()),
            };
            let (band, _) = regions.region("titlebar")?;
            Some(Rectangle::new(outer.loc + band.loc, band.size))
        }
        Found::Surface(id, output) => instance_area(state, *id, output),
        Found::LayerShell(surface) => state.space.outputs().find_map(|output| {
            let at = layer_map_for_output(output).layer_geometry(surface)?;
            let screen = state.space.output_geometry(output)?;
            Some(Rectangle::new(screen.loc + at.loc, at.size))
        }),
    }
}

/// The monitor a part is its own on: a pane's by its centre (where it is
/// drawn while it moves, `moved`), a scripted surface's instance's, a layer
/// surface's map's. What `effects.xray.straddle = "own"` keeps.
/// `state::tests::real_client::a_window_across_two_monitors_has_a_t2_place_on_each`.
fn own_monitor(
    state: &Solium,
    owner: &crate::effect::plan::Owner,
    found: &Found,
    moved: Option<Rectangle<f64, Logical>>,
) -> Option<String> {
    use crate::effect::plan::Owner as Whose;
    if let (Whose::Pane(..), Some(moved)) = (owner, moved) {
        return state
            .output_of(moved.to_i32_round())
            .map(|output| output.name());
    }
    match (owner, found) {
        (Whose::Surface(_, name), _) => Some(name.clone()),
        (_, Found::LayerShell(surface)) => state
            .space
            .outputs()
            .find(|output| {
                layer_map_for_output(output)
                    .layer_geometry(surface)
                    .is_some()
            })
            .map(smithay::output::Output::name),
        (Whose::Pane(pane, _), _) => state
            .pane_outer_of(*pane)
            .and_then(|outer| state.output_of(outer))
            .map(|output| output.name()),
        (Whose::LayerShell(_), _) => None,
    }
}

/// A T2 slot's places: one per monitor its padded box (the part where it
/// is `measured`, grown by `pad` physical pixels at `scale`, its own
/// monitor's) overlaps, or only its own monitor's under
/// `effects.xray.straddle = "own"`. Each holds its box in the xray's pixels
/// and in the monitor's own, which the T2 key reads, and what its run reads
/// there: `part`, the slot's own box, at rest; anywhere else the drawn
/// part's own pixels on that monitor, padded as the slot is, its radii
/// scaled with the picture (Ruling F37, amended 2026-10-14).
/// `state::tests::real_client::a_window_across_two_monitors_has_a_t2_place_on_each`,
/// `state::tests::real_client::a_drag_finer_than_an_xray_pixel_moves_the_t2_key_at_half_scale`,
/// `state::tests::real_client::a_gliding_windows_t2_place_follows_its_drawn_rectangle`.
fn t2_places(
    state: &Solium,
    owner: &crate::effect::plan::Owner,
    found: &Found,
    (cut, scale, pad): (crate::effect::xray::Cut, f64, i32),
    measured: &Measured,
    part: crate::effect::plan::PartBox,
) -> Vec<crate::effect::plan::T2Place> {
    let (rect, zoom) = match measured {
        Measured::Drawn(rect, zoom) => (*rect, *zoom),
        Measured::Rest(rect) => (rect.to_f64(), 1.0),
        Measured::Beneath(rect) => (*rect, 1.0),
    };
    let at_rest = matches!(measured, Measured::Rest(_));
    let grow = f64::from(pad) / scale.max(f64::EPSILON);
    let padded = Rectangle::<f64, Logical>::new(
        (rect.loc.x - grow, rect.loc.y - grow).into(),
        (rect.size.w + 2.0 * grow, rect.size.h + 2.0 * grow).into(),
    );
    let only = match state.effect_settings.xray.straddle {
        crate::effect::xray::Straddle::Each => None,
        crate::effect::xray::Straddle::Own => {
            Some(own_monitor(state, owner, found, (!at_rest).then_some(rect)))
        }
    };
    let mut places = Vec::new();
    for output in state.space.outputs() {
        let Some(geometry) = state.space.output_geometry(output) else {
            continue;
        };
        if !padded.overlaps(geometry.to_f64()) {
            continue;
        }
        let monitor = output.name();
        if only
            .as_ref()
            .is_some_and(|only| only.as_deref() != Some(monitor.as_str()))
        {
            continue;
        }
        let at = output.current_scale().fractional_scale();
        let xray = crate::offscreen::xray_size(state, output);
        let (px, map, own) = crate::effect::plan::place_on(padded, geometry, at, xray);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a zoom near 1, given to GL"
        )]
        let read = if at_rest {
            part
        } else {
            crate::effect::plan::PartBox::around(
                rect.size.to_physical_precise_round(at),
                pad,
                part.radii.map(|radius| radius * zoom as f32),
            )
        };
        places.push(crate::effect::plan::T2Place {
            monitor,
            cut,
            padded: px,
            screen: crate::effect::plan::screen_box(padded, geometry, at),
            map,
            own,
            part: read,
        });
    }
    places.sort_by(|a, b| a.monitor.cmp(&b.monitor));
    places
}

/// Where a T2 slot's places are measured from this pass (Ruling F37,
/// amended 2026-10-14, live under motion and warps).
/// `state::tests::real_client::a_gliding_windows_t2_place_follows_its_drawn_rectangle`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Measured {
    /// Where the walk draws a pane part away from its rest without a mesh,
    /// and the zoom its picture is drawn at: through a glide, a drag, an
    /// open, a `sol.present` rectangle.
    Drawn(Rectangle<f64, Logical>, f64),
    /// Where it rests ([`part_rect`], from `pane_outer`), today's place: a
    /// part that is not a pane's, a pane drawn at rest, a warped pane's
    /// slot kept in its capture, every warped slot under `"rest"`.
    Rest(Rectangle<i32, Logical>),
    /// A warped pane's behind slot drawn beneath its warp: the bounds of
    /// the part's cells of the mesh, global logical.
    Beneath(Rectangle<f64, Logical>),
}

/// A part's rest rectangle carried into the frame's rectangle: its offset
/// in the pane and its size scaled by the frame's size over the pane's.
/// `tests::a_glided_parts_box_is_where_it_is_drawn`.
pub(crate) fn within(
    part: Rectangle<i32, Logical>,
    outer: Rectangle<i32, Logical>,
    drawn: Rectangle<f64, Logical>,
) -> Rectangle<f64, Logical> {
    let across = drawn.size.w / f64::from(outer.size.w.max(1));
    let down = drawn.size.h / f64::from(outer.size.h.max(1));
    Rectangle::new(
        (
            drawn.loc.x + f64::from(part.loc.x - outer.loc.x) * across,
            drawn.loc.y + f64::from(part.loc.y - outer.loc.y) * down,
        )
            .into(),
        (
            f64::from(part.size.w) * across,
            f64::from(part.size.h) * down,
        )
            .into(),
    )
}

/// Where a wanted T2 slot's places are measured: a surface or a layer
/// surface where it is; a pane part under a warp beneath it (a behind slot
/// under `effects.xray.warped = "drawn"`, with neither program refused by
/// the driver, and under `straddle = "own"` only on one monitor) or at rest
/// (any other slot, or `"rest"`); a pane part without a warp where the walk
/// draws it ([`presented_part`]), which at rest is where it rests.
/// `state::tests::real_client::a_gliding_windows_t2_place_follows_its_drawn_rectangle`,
/// `state::tests::real_client::a_warped_panes_behind_slot_is_drawn_beneath_its_warp`,
/// `state::tests::real_client::a_warped_slot_across_two_monitors_under_own_stays_in_its_capture`.
pub(crate) fn measured(
    state: &Solium,
    owner: &crate::effect::plan::Owner,
    slot: Slot,
    found: &Found,
) -> Option<Measured> {
    let rest = part_rect(state, found)?;
    let crate::effect::plan::Owner::Pane(pane, part) = owner else {
        return Some(Measured::Rest(rest));
    };
    match state.presented.get(*pane) {
        Some(Planned::Warp { frame, grid, .. }) => {
            let beneath = state.effect_settings.xray.warped == crate::effect::xray::Warped::Drawn
                && slot.placed() == Slot::Behind
                && *part != PaneSlot::Popups
                && !state.programs.warp_refused()
                && !state.programs.beneath_refused();
            if !beneath {
                return Some(Measured::Rest(rest));
            }
            let outer = state.pane_outer_of(*pane)?;
            let (w, h) = (
                f64::from(outer.size.w.max(1)),
                f64::from(outer.size.h.max(1)),
            );
            let unit = crate::warp::UnitRect {
                u0: f64::from(rest.loc.x - outer.loc.x) / w,
                v0: f64::from(rest.loc.y - outer.loc.y) / h,
                u1: f64::from(rest.loc.x - outer.loc.x + rest.size.w) / w,
                v1: f64::from(rest.loc.y - outer.loc.y + rest.size.h) / h,
            };
            let bounds = beneath_bounds(frame, grid.as_ref(), unit)?;
            // One place cannot feed the draw beneath a second monitor, whose
            // hole would show the screen below: under "own" a warp across
            // monitors keeps the slot in its capture, the cheaper rule that
            // setting asks for (amended 2026-10-14, live under motion and warps).
            // `state::tests::real_client::a_warped_slot_across_two_monitors_under_own_stays_in_its_capture`.
            if state.effect_settings.xray.straddle == crate::effect::xray::Straddle::Own
                && monitors_overlapped(state, bounds) > 1
            {
                return Some(Measured::Rest(rest));
            }
            Some(Measured::Beneath(bounds))
        }
        _ => presented_part(state, *pane, *part, rest),
    }
}

/// How many monitors a rectangle overlaps with a positive area, as
/// [`t2_places`] asks of each.
fn monitors_overlapped(state: &Solium, rect: Rectangle<f64, Logical>) -> usize {
    state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .filter(|geometry| rect.overlaps(geometry.to_f64()))
        .count()
}

/// A pane part's bounds under a warp, global logical: with a grid (a
/// geometry folder's mesh, a present deform) the bounds of the part's
/// cells of it; with a matrix alone (a tilt, an open or close that warps:
/// one cell, the pane's corners) the part's own quad, projected exactly as
/// `warp::mesh_part` projects a part, so a titlebar's bounds are its own
/// band and not the whole pane's box (amended 2026-10-14, live under motion
/// and warps). `unit` is in the pane's own unit square, whatever part the
/// grid is over.
/// `tests::a_tilted_parts_beneath_bounds_are_its_own_band`,
/// `tests::a_parts_beneath_bounds_on_a_grid_over_the_bleed_are_its_own_cells`,
/// `warp::tests::a_parts_bounds_on_a_mesh_are_its_cells`.
pub(crate) fn beneath_bounds(
    frame: &present::Frame,
    grid: Option<&crate::warp::Grid>,
    unit: crate::warp::UnitRect,
) -> Option<Rectangle<f64, Logical>> {
    // Global logical: a "screen" at the origin, at scale 1.
    let origin = Rectangle::from_size((1, 1).into());
    let bounds = match grid {
        // The grid's texture runs 0 to 1 across its part, which is the pane
        // grown by its layers' bleed (#312): the part measured across it.
        Some(grid) => warp_mesh_on(origin, frame, Some(grid), grid.part, 1.0)?
            .part_bounds(unit.in_part(grid.part))?,
        // The part's own mesh: its texture runs 0 to 1 across the part, so
        // every one of its triangles is the part's.
        None => warp_mesh_on(origin, frame, None, unit, 1.0)?
            .part_bounds(crate::warp::UnitRect::WHOLE)?,
    };
    Some(Rectangle::new(
        (bounds.loc.x, bounds.loc.y).into(),
        (bounds.size.w, bounds.size.h).into(),
    ))
}

/// A pane part where the walk draws it this pass without a mesh, from the
/// frame `elements` draws the pane with (`state.drawn_id_at` over
/// `pane_outer_of` at the pass's instant, the call `stacked` makes): the
/// client and its popups through the walk's own [`place_client`], every
/// other part carried by the frame's rectangle ([`within`]); and the zoom
/// (the client's fit, else the frame's width over the pane's), which
/// scales the part's radii. At rest, today's place exactly.
/// `state::tests::real_client::a_gliding_windows_t2_place_follows_its_drawn_rectangle`,
/// `state::tests::real_client::a_gliding_windows_t2_place_is_measured_at_the_passs_instant`.
fn presented_part(
    state: &Solium,
    pane: crate::pane::PaneId,
    part: PaneSlot,
    rest: Rectangle<i32, Logical>,
) -> Option<Measured> {
    let held = state.panes.get(pane)?;
    let outer = state.pane_outer_of(pane)?;
    let frame = state.drawn_id_at(pane, outer, pass_now(state));
    if frame.rect == outer.to_f64() {
        return Some(Measured::Rest(rest));
    }
    let zoom = frame.rect.size.w / f64::from(outer.size.w.max(1));
    match part {
        PaneSlot::Client | PaneSlot::Popups => {
            let window = held.client()?;
            let real = state.real_geometry(window)?;
            let placed = place_client(state, held, &frame, outer.size, real.size);
            let rect = match part {
                PaneSlot::Popups => {
                    let covered = popups_covered(window)?;
                    Rectangle::new(
                        (
                            placed.origin.x + f64::from(covered.loc.x) * placed.fit.factor.x,
                            placed.origin.y + f64::from(covered.loc.y) * placed.fit.factor.y,
                        )
                            .into(),
                        (
                            f64::from(covered.size.w) * placed.fit.factor.x,
                            f64::from(covered.size.h) * placed.fit.factor.y,
                        )
                            .into(),
                    )
                }
                _ => placed.client,
            };
            Some(Measured::Drawn(rect, placed.fit.factor.x))
        }
        _ => Some(Measured::Drawn(within(rest, outer, frame.rect), zoom)),
    }
}

/// After [`record_boxes`]: each T2 place's cut wanted, and its memos judged
/// together under `effects.memo.budget_mib` (`memo::Memos::admit_pass`,
/// once a pass), a place that read its memos last pass first; an admitted
/// place's memos wanted with the plan their key was made from, a refused
/// place unshared (it runs its whole plan). Every cut and memo nobody reads
/// this pass is not wanted. The memos wanted go into `slots` for the
/// `xray` phase.
/// `state::tests::real_client::xray_is_wanted_only_on_a_monitor_a_t2_slot_reads`,
/// `state::tests::real_client::two_windows_with_one_blur_want_one_memo`,
/// `state::tests::real_client::over_the_memo_budget_a_slot_runs_its_whole_plan`,
/// `state::tests::real_client::two_parts_under_a_one_memo_budget_do_not_trade_it_as_their_keys_move`.
pub(crate) fn want_t2(state: &mut Solium, slots: &mut Slots, pass: u64) {
    use crate::effect::memo::{Ask, MemoKey};
    state.xrays.clear_wants();
    if slots.is_empty() {
        slots.set_memos(Vec::new());
        return;
    }
    let mut placed: Vec<(
        crate::effect::plan::Owner,
        Slot,
        crate::effect::rules::RuleKey,
        String,
    )> = Vec::new();
    let mut asks: Vec<Ask> = Vec::new();
    let wanted: Vec<_> = slots
        .wants()
        .map(|(owner, slot, key)| (owner.clone(), slot, key))
        .collect();
    for (owner, slot, key) in wanted {
        let Some(chain) = state.chains.get(key) else {
            continue;
        };
        let Some(memo) = chain.memo() else {
            continue;
        };
        for place in slots.places(&owner, slot) {
            state.xrays.want(&place.monitor, place.cut);
            let size = output_named(state, &place.monitor).map_or((1, 1), |output| {
                let xray = crate::offscreen::xray_size(state, &output);
                (
                    u32::try_from(xray.w).unwrap_or(1),
                    u32::try_from(xray.h).unwrap_or(1),
                )
            });
            let memos = memo
                .prefix
                .as_ref()
                .map(|prefix| {
                    memo.exports
                        .iter()
                        .zip(chain.memo_keys())
                        .map(|(export, upto)| {
                            let key = MemoKey {
                                monitor: place.monitor.clone(),
                                cut: place.cut,
                                upto: *upto,
                            };
                            let bytes =
                                crate::effect::memo::plan_bytes(&prefix.upto(export.step), size);
                            (key, bytes)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let read_last_pass = state
                .store
                .get(&owner, slot)
                .filter(|held| held.rule == key)
                .and_then(|held| held.t2.get(&place.monitor))
                .is_some_and(|t2| t2.read_last_pass(pass));
            asks.push(Ask {
                memos,
                read_last_pass,
            });
            placed.push((owner.clone(), slot, key, place.monitor.clone()));
        }
    }
    let answers = state.memos.admit_pass(
        &asks,
        state.effect_settings.memo.budget_mib,
        pass,
        &mut state.pool,
    );
    let mut wanted_memos: Vec<MemoKey> = Vec::new();
    for (((owner, slot, key, monitor), ask), admitted) in placed.into_iter().zip(asks).zip(answers)
    {
        if !admitted {
            slots.set_unshared(owner, slot, &monitor);
            continue;
        }
        if let Some(chain) = state.chains.get(key)
            && let Some(prefix) = chain.memo().and_then(|memo| memo.prefix.as_ref())
        {
            let steps: Vec<usize> = chain
                .memo()
                .map(|memo| memo.exports.iter().map(|export| export.step).collect())
                .unwrap_or_default();
            for ((memo_key, _), step) in ask.memos.iter().zip(steps) {
                state.memos.set_plan(memo_key, || prefix.upto(step));
                if !wanted_memos.contains(memo_key) {
                    wanted_memos.push(memo_key.clone());
                }
            }
        }
        state
            .store
            .slot_mut(&owner, slot, key)
            .t2_mut(&monitor)
            .read_at = Some(pass);
    }
    wanted_memos.sort();
    slots.set_memos(wanted_memos);
}

/// Run each memo of `run` (`memos_to_run`'s) over its cut: its plan, the
/// prefix up to the export it holds, at the xray's size, `backdrop` the
/// cut's picture, held in the memo. `Done` moves its generation and counts
/// `memo_runs`; `Pending` and `Failed` leave it with no result, so it runs
/// again. One `begin` and one `end` around them (Ruling 10).
/// `tests::a_memo_runs_when_its_xray_was_redrawn_and_once_for_every_slot`;
/// what it draws is wirecheck 13a's (Task 45).
pub(crate) fn run_xray(
    cx: &mut RunCx<'_>,
    pool: &mut crate::pool::Pool,
    runner: &mut dyn Runner,
    run: &[crate::effect::memo::MemoKey],
) {
    use crate::effect::run::{BoxMap, Inputs, Keys, Outcome};
    use smithay::backend::renderer::Texture as _;
    let mut began = false;
    for key in run {
        let (Some(generation), Some(texture)) = (
            cx.xrays.generation(&key.monitor, key.cut),
            cx.xrays.texture(&key.monitor, key.cut).cloned(),
        ) else {
            continue;
        };
        let size = texture.size();
        let textures = [("backdrop", texture, BoxMap::WHOLE)];
        let inputs = Inputs {
            padded: (
                u32::try_from(size.w).unwrap_or(1),
                u32::try_from(size.h).unwrap_or(1),
            ),
            content: [0.0, 0.0, 1.0, 1.0],
            textures: &textures,
            radii: [0.0; 4],
            time: cx.now,
            transition: crate::effect::run::Transition::default(),
            clamp_first: false,
        };
        let keys = Keys {
            params: key.upto,
            own: None,
            shape: 0,
        };
        let effects = cx.effects;
        let outcome = {
            let Some(memo) = cx.memos.get_mut(key) else {
                continue;
            };
            let crate::effect::memo::Memo { plan, held, .. } = memo;
            let Some(plan) = plan.as_ref() else {
                continue;
            };
            if !began {
                runner.begin();
                began = true;
            }
            runner.run(
                pool,
                &|program| effects.lookup(program),
                effects.formats(),
                plan,
                held,
                &inputs,
                &keys,
            )
        };
        match outcome {
            Outcome::Done(_, sync) => {
                let _ = crate::offscreen::settle(
                    Ok::<_, std::convert::Infallible>(sync),
                    crate::dev::fence_wait(),
                    "a memo",
                );
                cx.memos.ran(key, generation);
                crate::pacing::memo_ran();
            }
            // No result: it runs again on a later pass, and the places that
            // read it draw nothing behind their part meanwhile ([16] §5).
            Outcome::Pending | Outcome::Failed => {
                if let Some(memo) = cx.memos.get_mut(key) {
                    memo.made = false;
                }
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

/// Whether a piece is the client's own pixels (#296): the client, and the
/// slots on it and on its popups, which are its pixels run through an
/// effect. A layer's and the whole pane's slots are not.
fn is_client(piece: Piece) -> bool {
    matches!(
        piece,
        Piece::Client | Piece::Slot(PaneSlot::Client | PaneSlot::Popups, _)
    )
}

/// The alpha one of a pane's pieces is drawn at, the pane at `opacity` and
/// its client at `client_opacity` inside it (#296): the client's pixels at
/// both, the style's layers and the pane's own slots at the pane's alone, so
/// a layer `behind` the client shows through as the client fades. A capture
/// passes an `opacity` of 1, since its warp fades the whole texture after.
/// `tests::a_client_faded_out_draws_its_panes_layers_and_no_client`.
pub(crate) fn piece_alpha(opacity: f32, client_opacity: f32, piece: Piece) -> f32 {
    if is_client(piece) {
        opacity * client_opacity
    } else {
        opacity
    }
}

/// What the flat path draws a piece at under `frame`: [`piece_alpha`], or
/// nothing for the client's pixels once they show nothing
/// ([`present::Frame::shows_client`]), so a client faded into its pane
/// costs nothing to draw while its layers go on being drawn.
/// `tests::a_client_faded_out_draws_its_panes_layers_and_no_client`.
pub(crate) fn drawn_alpha(frame: &present::Frame, piece: Piece) -> Option<f32> {
    if is_client(piece) && !frame.shows_client() {
        return None;
    }
    Some(piece_alpha(frame.opacity, frame.client_opacity, piece))
}

/// A pane's pieces with its slots, topmost first (Ruling 15): the pane's
/// `front`, then `PANE_ORDER` with the client's slots around the client, then
/// the pane's `behind`; a pane `replace` takes the place of all of
/// `PANE_ORDER`, and a client `replace` the client's, each [`bracket`]ed by
/// its part's `front` and `behind` as every other part is (#389: a client's
/// `front` over its `replace` was left out, so an outline over a window made
/// see-through drew nothing). Layer and titlebar slots are inside `Layers`,
/// around their layer ([`around_layer`]), and the popups' around the
/// popups, which go in ahead of the walk.
/// `tests::with_no_slots_the_pane_walk_is_pane_order`,
/// `tests::client_slots_bracket_the_client_and_replace_takes_its_place`,
/// `tests::pane_slots_are_around_the_sandwich_and_below_the_popups`.
pub(crate) fn pane_walk<T>(
    into: &mut Vec<T>,
    has: &dyn Fn(PaneSlot, Slot) -> bool,
    mut piece: impl FnMut(&mut Vec<T>, Piece),
) {
    if has(PaneSlot::Pane, Slot::Front) {
        piece(into, Piece::Slot(PaneSlot::Pane, Slot::Front));
    }
    if has(PaneSlot::Pane, Slot::Replace) {
        piece(into, Piece::Slot(PaneSlot::Pane, Slot::Replace));
    } else {
        for each in PANE_ORDER {
            match each {
                Piece::Client => bracket(
                    |slot| has(PaneSlot::Client, slot),
                    |slot| match slot {
                        Some(slot) => piece(into, Piece::Slot(PaneSlot::Client, slot)),
                        None => piece(into, Piece::Client),
                    },
                ),
                other => piece(into, other),
            }
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
/// walk asks; on `monitor` a T2 slot is ready where it has that monitor's
/// result (`effect::plan::tests::a_monitors_own_result_wins_over_the_slots_one`).
/// `tests::a_wanted_slot_with_nothing_ready_draws_what_no_slot_draws`.
pub(crate) fn slot_ready<'a>(
    slots: &'a Slots,
    pane: crate::pane::PaneId,
    monitor: Option<&'a str>,
) -> impl Fn(PaneSlot, Slot) -> bool + 'a {
    move |part, slot| {
        let owner = crate::effect::plan::Owner::Pane(pane, part);
        match monitor {
            // On one monitor, a T2 slot's result there or the slot's one.
            Some(monitor) => slots.is_ready_on(&owner, slot, monitor),
            None => slots.is_ready(&owner, slot),
        }
    }
}

/// A warped pane's capture: every slot ready this pass, in the flat path's
/// order. `tests::a_warped_panes_capture_walks_its_client_slot`.
pub(crate) fn capture_pieces(slots: &Slots, pane: crate::pane::PaneId) -> Vec<Piece> {
    let mut pieces = Vec::new();
    pane_walk(
        &mut pieces,
        &slot_ready(slots, pane, None),
        |into, piece| {
            into.push(piece);
        },
    );
    pieces
}

/// A whole pane's self capture: its inner parts' results, never its own
/// `pane` slots, which would read the last pass's result of the effect being
/// run. `tests::a_whole_panes_capture_walks_its_inner_slots_and_not_its_own`.
pub(crate) fn inner_pieces(slots: &Slots, pane: crate::pane::PaneId) -> Vec<Piece> {
    let ready = slot_ready(slots, pane, None);
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

/// Which walk a slot is placed in: the frame's own, or a pane's capture.
/// `tests::a_beneath_slot_is_a_hole_only_inside_a_capture`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Within {
    Screen,
    Capture,
}

/// What a ready slot draws in a walk.
/// `tests::a_beneath_slot_is_a_hole_only_inside_a_capture`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotDraw {
    /// Its result, over its part.
    Result,
    /// A hole in its part's shape: the slot is drawn beneath the pane's
    /// warp, and what the pane's lower layers drew there is taken out.
    Hole,
    /// Nothing: a beneath slot with no texture on some monitor this pass
    /// (its memo pending, its run failed), drawn as the flat path draws a
    /// slot not ready there, the pane's lower layers showing, and never a
    /// hole with nothing beneath it (amended 2026-10-14, live under motion
    /// and warps).
    Skip,
}

/// A beneath slot is a hole inside its pane's capture and nowhere else,
/// and only while every one of its places has its texture this pass
/// (`Slots::screened_all`): the capture is one picture for every monitor
/// the warp is drawn on, so a monitor without its `Under` would show the
/// screen below through the hole. Both captures of a pane are inside: the
/// warp's, and a whole pane's self or mask capture, whose result lies over
/// the client's hole in the warp's capture (Ruling F37).
/// `tests::a_beneath_slot_is_a_hole_only_inside_a_capture`.
pub(crate) fn slot_draw(within: Within, beneath: bool, screened: bool) -> SlotDraw {
    match (within, beneath, screened) {
        (Within::Capture, true, true) => SlotDraw::Hole,
        (Within::Capture, true, false) => SlotDraw::Skip,
        _ => SlotDraw::Result,
    }
}

/// A ready slot's result placed over its part, `part` being the part's
/// mask on this output in logical pixels (`effect::mask`), cut by it as
/// [`cut_by_shape`] says, at `alpha`; nothing for a slot with no result on
/// `monitor`, the output it is drawn on. In a capture, a slot drawn beneath
/// its pane's warp is a hole, or nothing ([`slot_draw`]). A slot
/// crossfading between two rules is both their results mixed in one draw
/// (`effect::crossfade`, #389).
#[expect(
    clippy::too_many_arguments,
    reason = "one slot placed: whose, which, over what, on which monitor and in which walk, at what scale and alpha"
)]
fn slot_element(
    state: &Solium,
    slots: &Slots,
    owner: &crate::effect::plan::Owner,
    slot: Slot,
    part: Mask,
    scale: f64,
    alpha: f32,
    (monitor, within): (Option<&str>, Within),
) -> Option<Element> {
    match slot_draw(
        within,
        slots.is_beneath(owner, slot),
        slots.screened_all(owner, slot),
    ) {
        SlotDraw::Result => {}
        SlotDraw::Skip => return None,
        // A known limit (#297): the hole erases what the pane's lower layers
        // drew under it at full strength, so a client backdrop slot under a
        // warp erases the layers beneath it at any `client_opacity` (#296) --
        // a style's icon `behind` the client included -- where the flat path
        // fades the slot with the client. Its `Beneath` stays at the pane's
        // alpha to match, rather than leave a see-through hole.
        SlotDraw::Hole => {
            let Mask::Rect { rect, radii } = part else {
                return None;
            };
            let program = state.programs.beneath_compiled()?;
            let id = state.store.get(owner, slot)?.hole.clone();
            return Some(Element::Hole(crate::effect::beneath::Hole::new(
                owner.clone(),
                slot,
                id,
                rect.to_physical(scale),
                radii_of(corners_times(radii, scale)),
                program,
            )));
        }
    }
    // A slot fading in or out with its rule is drawn at its fade's opacity,
    // its result as it is (`effect::fade`).
    let alpha = alpha * slots.opacity(owner, slot);
    let (share, raw) = (slots.mix(owner, slot), slots.raw(owner, slot));
    if share.is_none() && raw.is_none() {
        return placed_result(state, slots, owner, slot, part, scale, alpha, monitor)
            .map(Element::Effect);
    }
    // Crossfading: from the rule in its `Leaving` twin, and for a `replace`
    // whose rule comes or goes, with its part's own pixels, all mixed in one
    // draw (`effect::crossfade`). A part with no capture of its own pixels
    // (the titlebar) has its result faded over nothing instead.
    let part_pixels = raw.and_then(|_| part_picture(state, slots, owner, slot, part, scale, alpha));
    let (alpha, raw) = match (raw, &part_pixels) {
        (Some(raw), None) => (alpha * (1.0 - raw), 0.0),
        (raw, _) => (alpha, raw.unwrap_or(0.0)),
    };
    let own = placed_result(state, slots, owner, slot, part, scale, alpha, monitor);
    let leaving = share.and_then(|_| {
        placed_result(
            state,
            slots,
            owner,
            slot.leaving(),
            part,
            scale,
            alpha,
            monitor,
        )
    });
    let results = 1.0 - raw;
    let mut pictures = Vec::with_capacity(3);
    match (own, leaving) {
        (Some(own), Some(leaving)) => {
            let share = share.unwrap_or(1.0);
            pictures.push((own, results * share));
            pictures.push((leaving, results * (1.0 - share)));
        }
        (Some(one), None) | (None, Some(one)) => pictures.push((one, results)),
        (None, None) => {}
    }
    pictures.extend(part_pixels.map(|pixels| (pixels, raw)));
    let program = state.programs.crossfade_compiled();
    match (pictures.len(), program) {
        (2.., Some(program)) => {
            crate::effect::crossfade::CrossfadeElement::new(&pictures, program.clone())
                .map(Element::Crossfade)
        }
        // One picture, or no program to mix them: the first, the slot's own
        // result where it has one, at once.
        _ => pictures
            .into_iter()
            .next()
            .map(|(first, _)| Element::Effect(first)),
    }
}

/// A `replace` slot's part's own pixels, as captured for it this pass, placed
/// over the part as its result is and cut by the part's shape, at `alpha`:
/// what the slot mixes its result with as its rule comes and goes. `None`
/// for a part with no capture.
fn part_picture(
    state: &Solium,
    slots: &Slots,
    owner: &crate::effect::plan::Owner,
    slot: Slot,
    part: Mask,
    scale: f64,
    alpha: f32,
) -> Option<crate::effect::element::EffectElement> {
    use smithay::backend::renderer::Texture as _;
    let key = slots.key(owner, slot)?;
    let chain = state.chains.get(key)?;
    let held = state
        .store
        .get(owner, slot)
        .filter(|held| held.rule == key)?;
    let texture = held.input.target()?.texture().clone();
    let (_, part_scale) = part_of(state, owner)?;
    let Mask::Rect { rect, radii } = part else {
        return None;
    };
    let size = texture.size();
    let (dst, mask) = slot_placement(
        rect.to_physical(scale),
        (size.w, size.h).into(),
        padding(chain.reach, part_scale),
        corners_times(radii, scale),
    );
    let program = state.programs.masked_compiled()?.clone();
    Some(
        crate::effect::element::EffectElement::new(
            held.input.id().clone(),
            held.input.commit(),
            texture,
            program,
        )
        .at(dst, Some(mask), alpha),
    )
}

/// One rule's result in a slot (a crossfading slot's `Leaving` twin's
/// included), placed over its part as [`slot_element`] says, at `alpha`;
/// nothing for a slot with no result on `monitor`.
#[expect(
    clippy::too_many_arguments,
    reason = "one result placed: whose, which, over what, at what scale and alpha, on which monitor"
)]
fn placed_result(
    state: &Solium,
    slots: &Slots,
    owner: &crate::effect::plan::Owner,
    slot: Slot,
    part: Mask,
    scale: f64,
    alpha: f32,
    monitor: Option<&str>,
) -> Option<crate::effect::element::EffectElement> {
    // On `monitor`, its own result there (a T2 slot's) or the slot's one
    // (`effect::plan::tests::a_monitors_own_result_wins_over_the_slots_one`).
    let ready = match monitor {
        Some(monitor) => slots.at_on(owner, slot, monitor),
        None => slots.at(owner, slot),
    }?;
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
    Some(ready.element.at(dst, cut.then_some(mask), alpha))
}

/// The ready slots around a pane's layers at `depth`, placed, each with its
/// layer's place and where it goes ([`around_layer`]): what `chrome`'s hook
/// pushes ([`place_around`]), made before the decoration is borrowed to draw
/// them.
fn layer_slots(
    state: &Solium,
    slots: &Slots,
    (monitor, within): (Option<&str>, Within),
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
            let ready = match monitor {
                Some(monitor) => slots.is_ready_on(&owner, slot, monitor),
                None => slots.is_ready(&owner, slot),
            };
            if !ready {
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
                    (monitor, within),
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
#[expect(
    clippy::too_many_arguments,
    reason = "the frame's own state, and the slots and monitor its layers' slots are placed from"
)]
fn chrome(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    elements: &mut Vec<Element>,
    slots: Option<&Slots>,
    monitor: (Option<&str>, Within),
    pane: crate::pane::PaneId,
    depth: Depth,
    drawing: crate::decoration::Drawing,
) {
    let title = state.pane_title(pane);
    let app_id = state.pane_app_id(pane);
    let look = crate::decoration::Look {
        title: &title,
        app_id: &app_id,
        focused: state.looks_focused(pane),
        pointer_inside: state.pointer_over(pane),
        // `text_input::tests::only_the_pane_whose_window_has_the_caret_is_given_it`.
        caret: state.caret_in(pane),
        // `decoration::tests::a_layer_is_told_the_configurations_values_and_told_again_when_they_change`.
        values: state.decorations.values(),
    };
    let mut around = slots
        .map(|slots| layer_slots(state, slots, monitor, pane, depth, drawing))
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
        // Only the canvas is asked of it, which no instant changes.
        now: std::time::Duration::ZERO,
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
    let drawing = layer_drawing_at(canvas, outer, at, scale, pass_now(state));
    let title = state.pane_title(pane);
    let app_id = state.pane_app_id(pane);
    let look = crate::decoration::Look {
        title: &title,
        app_id: &app_id,
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
    now: std::time::Duration,
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
        now,
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
#[expect(
    clippy::too_many_arguments,
    reason = "the frame's own state, and the pane, its transform and its own size"
)]
fn scene(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    elements: &mut Vec<Element>,
    pane: crate::pane::PaneId,
    frame: present::Frame,
    outer: Size<i32, Logical>,
    now: std::time::Duration,
    scale: f64,
) {
    let waited = state.panes.get(pane).map_or(0, |held| {
        i32::try_from(held.waited(now).as_millis()).unwrap_or(i32::MAX)
    });
    // Two fades, and they multiply. The scene's own, as the application it was
    // standing in for appears underneath it; and the pane's, because a window
    // being closed or animated by a script takes everything inside it along --
    // the scene is part of the window, not something laid over it. The pane's
    // is the client's (#296): the scene stands in for the client, so it fades
    // with `client_opacity` as the client would.
    let fade = state.loading.fade;
    let alpha = frame.client_alpha()
        * state
            .panes
            .get(pane)
            .map_or(1.0, |held| held.scene_alpha(now, fade));
    let (laid_out, drawn) = standing_layout(outer, &frame);
    let Some(held) = state.panes.get_mut(pane).and_then(Pane::scene_mut) else {
        return;
    };
    held.set_int("waited", waited);
    // Already an `Element`: a shell surface is a memory buffer on the software
    // path and a texture on the GPU one, and which of the two it is is its own
    // business rather than this function's.
    elements.extend(
        held.element_over(
            renderer,
            laid_out,
            drawn,
            now,
            alpha,
            crate::surface::Raster::at(scale),
        )
        .element,
    );
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

/// A pane's standing-in scene as a warp's capture holds it: over the
/// pane's whole rectangle at its own size, its corner at `at` in the
/// target's physical pixels (past the layers' bleed, #312), as the walk
/// draws it over the pane's frame rectangle, unfaded by the frame (the warp
/// fades the whole capture) and faded by its own dissolve into the arriving
/// application. Nothing for a pane without one.
pub(crate) fn scene_in_capture(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    pane: crate::pane::PaneId,
    (outer, at): (Size<i32, Logical>, Point<i32, Physical>),
    scale: f64,
    client_opacity: f32,
) -> Vec<Element> {
    let mut elements = Vec::new();
    if state.pane_has_scene(pane) {
        let mut at_rest = present::Frame::real(Rectangle::from_size(outer));
        // The client's opacity in the pane, which the scene stands in for
        // (#296): a launch's scene fading in over its icon layer.
        at_rest.client_opacity = client_opacity;
        at_rest.rect.loc = at.to_f64().to_logical(scale);
        let now = pass_now(state);
        scene(
            state,
            renderer,
            &mut elements,
            pane,
            at_rest,
            outer,
            now,
            scale,
        );
    }
    elements
}

/// What a window still loading is captured from for its warp (#302): the
/// walk's `ours` branch at rest, the pane's corner at `at` in the target's
/// physical pixels (past the layers' bleed, #312), in `PANE_ORDER`,
/// the style's layers at their depths when `loading.decorated` draws them
/// and the standing-in scene in the client's place
/// ([`scene_in_capture`]), so it bends as one thing and an `above` layer
/// covers the scene as it will cover the client.
/// `state::tests::real_client::a_loading_pane_under_a_present_is_planned_through_the_warp`
/// says what is captured ([`captured`]); building it needs a renderer.
pub(crate) fn standing_elements(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    pane: crate::pane::PaneId,
    at: Point<i32, Physical>,
    scale: f64,
    client_opacity: f32,
) -> Vec<Element> {
    let Some(outer) = state.pane_outer_of(pane).map(|outer| outer.size) else {
        return Vec::new();
    };
    let (drawing, _) = pane_drawing_at(outer, state.insets_of(pane), at, scale, pass_now(state));
    let mut scene = Some(scene_in_capture(
        state,
        renderer,
        pane,
        (outer, at),
        scale,
        client_opacity,
    ));
    let mut elements = Vec::new();
    pane_pieces(&mut elements, |elements, piece| match piece {
        Piece::Layers(depth) if state.loading.decorated => {
            chrome(
                state,
                renderer,
                elements,
                None,
                (None, Within::Capture),
                pane,
                depth,
                drawing,
            );
        }
        Piece::Layers(_) | Piece::Slot(..) => (),
        Piece::Client => elements.extend(scene.take().into_iter().flatten()),
    });
    elements
}

/// What a pane's standing-in scene is laid out at, and where its picture is
/// drawn: the pane's own outer size, and the rectangle its frame presents
/// this instant, which it is scaled into.
///
/// **Its own size and not the presented one**, as the pane's layers are
/// (`Decoration::layer_elements`): a window zooming open, gliding to a new
/// tile, or fading out is drawn from the picture it has, and only a new
/// size of the window itself is a new picture. Laid out at the presented
/// rectangle, it was a new size on every frame of every such animation, and
/// on the GPU a new buffer each time: what made `loading/window` the costliest
/// scene of the 2026-10-15 TTY trace (504 renders, 3.1 ms each).
/// `tests::a_standing_in_scene_is_laid_out_at_its_windows_own_size`.
pub(crate) fn standing_layout(
    outer: Size<i32, Logical>,
    frame: &present::Frame,
) -> (Size<i32, Logical>, Rectangle<f64, Logical>) {
    ((outer.w.max(1), outer.h.max(1)).into(), frame.rect)
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
    /// Whether it is a capture (screencopy), not a monitor's frame: one never
    /// has the desktop under the lock's entry (`Solium::desktop_under_lock`).
    pub(crate) capture: bool,
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
            capture: false,
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
    // The first frame's gate holds: the frame is the clear colour, as a
    // monitor with nothing on it is today, until what it shows is ready.
    // No unit test reaches this: `dev/effects-check.sh xray`'s
    // `xray-first-on`, under `SOLIUM_WARM_INJECT=3`, finds its first three
    // frames only the clear colour, and fails with this return removed.
    if prepared.held {
        return Vec::new();
    }
    let Picture {
        screen,
        scale,
        cursor: with_cursor,
        capture,
    } = picture;
    // At the pass's one instant, the one its T2 places were measured at,
    // however late this runs (amended 2026-10-14, live under motion and
    // warps): `tests::the_walk_draws_at_the_passs_instant`.
    let now = walk_instant(prepared.now, state.clock.now());
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
            now,
        ));
    }

    // Solium's own lock has just ended: the session is unlocked, input is the
    // desktop's, and the desktop is drawn below; the lock's scene for this
    // monitor plays its exit over it, under only the pointer, until
    // `lock.exit_ms` is up, and is drawn no more after that whatever it is
    // doing (`lock_scene.rs`). A frame is asked for every frame of it, so the
    // bound is met even by a scene that does nothing.
    // `tests::the_lock_s_exit_is_drawn_over_the_session_and_under_the_pointer`.
    //
    // The same over a peek (`sol.lock_peek`): the scene pulled down over the
    // desktop by the configuration's own gesture, while the session is
    // unlocked and its input the session's. Frames are asked for as the
    // scene animates and as the peek moves, not every frame, so a peek left
    // standing costs nothing.
    if state.lock.is_none()
        && (state.lock_scenes.peeking().is_some() || state.lock_scenes.exiting(now))
        && let Some(output) = state.output_for(screen)
        && state.lock_scene_over_desktop(&output, now)
    {
        let painted = state.lock_scenes.element(
            renderer,
            &output,
            smithay::utils::Rectangle::from_size(screen.size),
            now,
            crate::surface::Raster::at(scale),
        );
        if painted.animating || state.lock_scenes.exiting(now) {
            state.redraw = true;
        }
        elements.extend(painted.element);
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
    //
    // Solium's own lock draws its scene for this monitor here instead, and the
    // backdrop goes under that too: a scene that has not drawn yet, or draws
    // with holes in it, shows the backdrop through them (`lock_scene.rs`).
    //
    // But for the entry of Solium's own lock: for `lock.enter_ms` its scene
    // comes down over the desktop, so on a monitor it plays on the session is
    // drawn under the scene rather than the backdrop, and this does not
    // return. Never for a capture, which is the lock's from its first
    // instant (`Solium::desktop_under_lock`). Input is the lock's all the
    // same, and the frames built meanwhile do not count as showing the lock.
    if let Some(lock) = state.lock.as_ref() {
        let blank = lock.blank();
        let native = lock.is_native();
        let output = state.output_for(screen);
        let desktop = output
            .as_ref()
            .is_some_and(|output| state.desktop_under_lock(output, now, capture));
        let client = output
            .as_ref()
            .and_then(|output| lock.surface_for(output))
            .map(|surface| surface.wl_surface().clone());
        if let Some(surface) = client {
            elements.extend(
                render_elements_from_surface_tree::<
                    GlesRenderer,
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(
                    renderer,
                    &surface,
                    (0, 0),
                    output_scale,
                    1.0,
                    Kind::Unspecified,
                )
                .into_iter()
                .map(Element::Window2),
            );
        } else if native && let Some(output) = output.as_ref() {
            let painted = state.lock_scenes.element(
                renderer,
                output,
                smithay::utils::Rectangle::from_size(screen.size),
                now,
                crate::surface::Raster::at(scale),
            );
            // Still moving, or swapped for the fallback: the next frame is
            // asked for, as `scripted` asks. Through the entry every frame
            // is, so its bound is met by a scene that does nothing.
            if painted.animating || desktop {
                state.redraw = true;
            }
            elements.extend(painted.element);
        }
        // Through the entry the session follows, under the scene, as it
        // would unlocked; otherwise the backdrop, and nothing more.
        if !desktop {
            elements.push(Element::Solid(
                smithay::backend::renderer::element::solid::SolidColorRenderElement::new(
                    blank,
                    smithay::utils::Rectangle::from_size(screen.size)
                        .to_physical_precise_round(scale),
                    CommitCounter::default(),
                    crate::lock::BLANK,
                    Kind::Unspecified,
                ),
            ));
            return elements;
        }
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
    // topmost first, and [`stacked_elements`] turns each entry into elements
    // and nothing more, with the slots beside a surface when any slot is
    // wanted ([`stacked_walk`]).
    let output = state.output_for(screen);
    // The monitor a T2 slot's result is placed from (Ruling F30), named only
    // while a slot is wanted (spec §8.4):
    // `effect::plan::tests::a_monitors_own_result_wins_over_the_slots_one`.
    let monitor = output
        .as_ref()
        .filter(|_| !prepared.slots.is_empty())
        .map(smithay::output::Output::name);
    let listed = stacked(state, screen, now);
    let mut draw = |each: Walked| match each {
        Walked::Stacked(each) => stacked_elements(
            state,
            renderer,
            prepared,
            output.as_ref(),
            each,
            screen,
            now,
            crate::surface::Raster::at(scale),
            &mut elements,
        ),
        Walked::Slot(owner, slot, rect, alpha) => {
            let rect = match owner {
                crate::effect::plan::Owner::LayerShell(_) => rect,
                crate::effect::plan::Owner::Surface(..) | crate::effect::plan::Owner::Pane(..) => {
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
                (monitor.as_deref(), Within::Screen),
            ));
        }
    };
    match monitor.as_deref() {
        Some(on) => stacked_walk(
            listed,
            on,
            &|owner, slot| prepared.slots.is_ready_on(owner, slot, on),
            &mut draw,
        ),
        None => listed.into_iter().map(Walked::Stacked).for_each(&mut draw),
    }

    elements
}

/// One entry of [`stacked`] as elements, pushed onto `elements`:
/// `elements`' arm for each entry, factored out so `elements` and an xray
/// draw one the same way (Ruling F29). A client's layer surface through its
/// own surface tree, at its place in its output's own coordinates; a
/// script's surface through [`scripted`], on `output`, which holds its
/// instance (none with no output, as before); panes through [`panes`], which
/// reads the pass's `prepared`. Pushed onto the caller's list, as [`panes`]
/// does, so the frame of an unstyled desktop allocates nothing it did not
/// before. Everything is placed at `raster.place`, the target's scale; a
/// script's surface is rasterised at `raster.scale`, its monitor's own
/// even inside an xray cut (`surface::Raster`).
/// `state::tests::real_client::reflow_on_close::stacking`'s tests show
/// `elements` unchanged; `offscreen::xray_job` draws a cut through it.
/// `state::tests::real_client::the_xray_rasterises_its_scenes_at_the_monitors_scale`
/// pins the raster a cut hands in, not this draw, which needs a renderer:
/// `dev/effects-check.sh xray`'s `xray-half` and `xray-half-anim` do, a
/// scene drawn at the cut's own size rendering up to three times a pass and
/// its still cut rebuilt on every pass.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame's own state, handed down from `elements` or an xray's job"
)]
pub(crate) fn stacked_elements(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    prepared: &Prepared,
    output: Option<&smithay::output::Output>,
    each: Stacked,
    screen: Rectangle<i32, Logical>,
    now: std::time::Duration,
    raster: crate::surface::Raster,
    elements: &mut Vec<Element>,
) {
    let scale = raster.place;
    match each {
        Stacked::Layer(surface, geometry) => {
            // A layer map's geometry is already in its own output's
            // coordinates, so these are the one thing on this list that
            // must *not* be shifted.
            let origin = geometry.loc.to_physical_precise_round(scale);
            let layer_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                surface.render_elements(renderer, origin, Scale::from(scale), 1.0);
            elements.extend(layer_elements.into_iter().map(|element| {
                Element::Window(RescaleRenderElement::from_element(
                    element,
                    origin,
                    Scale::from(1.0),
                ))
            }));
        }
        Stacked::Surface(id, area, alpha) => {
            if let Some(output) = output {
                elements.extend(scripted(
                    state, renderer, output, id, area, alpha, screen, now, raster,
                ));
            }
        }
        Stacked::Panes(nodes) => panes(
            state, renderer, prepared, elements, nodes, screen, now, scale,
        ),
    }
}

/// The band an entry of [`stacked`] is drawn in: a client's layer surface
/// at its layer, a script's surface at its declared one, and the windows'
/// band for panes. What an xray's cut is chosen by
/// (`effect::xray::XrayConfig::cut_below`) and its entries kept by
/// (`effect::xray::SessionLayers`). A script's surface no longer declared
/// is in no layer's band, so no cut holds it; [`stacked`] lists only
/// declared ones.
/// `state::tests::real_client::reflow_on_close::stacking::xray_holds_the_cuts_bands_and_nothing_else`.
pub(crate) fn band_of(state: &Solium, each: &Stacked) -> Band {
    match each {
        Stacked::Layer(surface, _) => Band::Layer(layer::ours(surface.layer()), Owner::Client),
        Stacked::Surface(id, ..) => state.surfaces.get(*id).map_or(Band::Windows, |surface| {
            Band::Layer(surface.layer(), Owner::Script)
        }),
        Stacked::Panes(_) => Band::Windows,
    }
}

/// One entry of a monitor's frame, before it is turned into elements.
/// `Clone` and `PartialEq` so a test compares an xray's entries with
/// [`stacked`]'s (`state::tests::real_client::reflow_on_close::stacking::xray_holds_the_cuts_bands_and_nothing_else`).
#[derive(Clone, Debug, PartialEq)]
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
        // A minimized window, once the present that took it away has landed,
        // is drawn nowhere (#30). See `Solium::minimized_away`.
        .filter(|(pane, _)| {
            !state
                .panes
                .get(*pane)
                .is_some_and(|held| state.minimized_away(held, now))
        })
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

/// Push `pane`'s warp, if `prepare` captured one this pass and its frame
/// is transformed: its popups' warp, its own, and the slots beneath it.
/// Whether anything of it was pushed; nothing is when its mesh fails here,
/// and the pane falls through to the flat path. What a window and a window
/// still loading are drawn through alike (#302).
///
/// A transform that is not identity cannot be drawn as a rectangle. The
/// window is rendered flat into a texture first — frame included — and
/// that texture is bent, so the whole window deforms as one thing
/// instead of the client tilting away from its own titlebar. The texture
/// holds what its style's layers paint past it, a shadow or a title above
/// it, and is drawn over the pane grown by that much ([`warp_part`]), so
/// they bend with it (#312). Its popups
/// are not in that texture (`state::tests::a_warped_panes_capture_holds_no_popups`):
/// they are a capture of their own, bent by the same matrix and deform
/// over their part of the pane, and drawn in front of it
/// (`tests::a_warped_panes_popups_are_in_front_of_it`).
///
/// The deform's anchor is resolved *here*, on the frame that draws it,
/// because what it is aimed at moves — see `present::Anchor`. An anchor
/// that resolves to nothing leaves `aimed` empty, and a window with no
/// matrix then takes the flat path as if it had never asked for an
/// effect. Its grid is the one `prepare` built this pass, once for every
/// output (`present_grid`).
#[expect(
    clippy::too_many_arguments,
    reason = "the walk's own state for one pane on one output"
)]
fn push_warp(
    state: &mut Solium,
    prepared: &Prepared,
    elements: &mut Vec<Element>,
    pane: crate::pane::PaneId,
    (frame, drawn_global): (present::Frame, present::Frame),
    screen: Rectangle<i32, Logical>,
    scale: f64,
    monitor: Option<&str>,
) -> bool {
    let aimed = state.aimed_at_for(pane, frame.deform);
    if (!frame.matrix.is_identity() || aimed.is_some())
        && let Some(warped) = prepared.warp(pane)
        && let Some(pane_mesh) = warp_mesh_on(
            screen,
            &drawn_global,
            warped.grid.as_ref(),
            warped.part,
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
                    // The popups are the client's (#296): at its alpha, and
                    // not at all once it shows nothing, as on the flat path.
                    if frame.shows_client()
                        && let Some(over) = prepared.over(pane)
                        && let Some(mesh) = warp_mesh_on(
                            screen,
                            &drawn_global,
                            over.grid.as_ref(),
                            over.part,
                            scale,
                        )
                    {
                        elements.push(Element::Warped(
                            crate::warp::Warp::new(
                                over.id.clone(),
                                over.commit,
                                over.texture.clone(),
                                mesh,
                                frame.client_alpha(),
                                over.program,
                            )
                            .with_pixels(over.pixels.clone()),
                        ));
                    }
                }
                WarpPiece::Pane => {
                    // Its pane capture's id for life, and the commit
                    // `prepare` moved only if the picture or the mesh
                    // changed, so a still warp is not damaged and a moving
                    // one is: `keyed::tests::a_warp_at_rest_keeps_its_commit`.
                    if let Some((id, texture, mesh)) = pane_warp.take() {
                        elements.push(Element::Warped(
                            crate::warp::Warp::new(
                                id,
                                commit,
                                texture,
                                mesh.clone(),
                                frame.opacity,
                                program,
                            )
                            .with_pixels(warped.pixels.clone()),
                        ));
                        // Beneath the warp, in its capture's order, through
                        // its mesh, each fragment reading the backdrop where
                        // it is on this monitor (Ruling F37, amended
                        // 2026-10-14): `dev/effects-check.sh motion`'s
                        // `genie`, and wirecheck case 13i.
                        if let Some(beneath) = state.programs.beneath_compiled() {
                            for each in warped
                                .under
                                .iter()
                                .filter(|each| monitor == Some(each.monitor.as_str()))
                            {
                                let map = match each.over {
                                    crate::effect::beneath::Over::Monitor => {
                                        crate::effect::beneath::ScreenMap::monitor(
                                            screen.size,
                                            scale,
                                        )
                                    }
                                    crate::effect::beneath::Over::Box(screen_box) => {
                                        crate::effect::beneath::ScreenMap::of_box(screen_box)
                                    }
                                };
                                elements.push(Element::Beneath(
                                    crate::effect::beneath::Beneath::new(
                                        each,
                                        mesh.clone(),
                                        map,
                                        frame.opacity,
                                        beneath,
                                    ),
                                ));
                            }
                        }
                    }
                }
            }
        }
        return true;
    }
    false
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
    // The monitor a T2 slot's result is placed from (Ruling F30), asked once
    // and only while a slot is wanted (spec §8.4):
    // `effect::plan::tests::a_monitors_own_result_wins_over_the_slots_one`.
    let monitor = if prepared.slots.is_empty() {
        None
    } else {
        state.output_for(screen).map(|output| output.name())
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
                now,
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
            now,
        };

        // Its client has gone: its frame's layers, fading with the rest of it,
        // round what the client left. In `PANE_ORDER`, like a live client.
        if remains {
            let mut client = Some(remains_elements(state, pane, &frame, outer.size, scale));
            pane_pieces(elements, |elements, piece| match piece {
                Piece::Layers(depth) => {
                    chrome(
                        state,
                        renderer,
                        elements,
                        None,
                        (None, Within::Screen),
                        pane,
                        depth,
                        drawing,
                    );
                }
                Piece::Client => elements.extend(client.take().into_iter().flatten()),
                Piece::Slot(..) => {}
            });
            continue;
        }

        // A refused present geometry under `failed = "hide"`: nothing of it
        // this pass, a window still loading included
        // (`tests::a_refused_present_follows_its_failed_policy`).
        if prepared.hidden(pane) {
            continue;
        }

        // Nothing of the application to draw yet, so this window is entirely
        // ours and the scene has all of it, bar included. The frame is built
        // and its room reserved — which is why the window does not change shape
        // when the application arrives — but drawing a bar over a surface that
        // already carries the name says it twice, so that is a setting and it
        // is off.
        //
        // Under a present that transforms it, it is drawn through its warp as
        // an arrived window is, from a capture of exactly this (#302): a
        // launch is mostly over before its application draws, so the scene
        // is the window for the whole of it.
        if ours
            && push_warp(
                state,
                prepared,
                elements,
                pane,
                (frame, drawn_global),
                screen,
                scale,
                monitor.as_deref(),
            )
        {
            continue;
        }
        if ours {
            // Through `PANE_ORDER` as well, so a pane waiting for its
            // application is layered the same way it will be once it arrives:
            // an `above` layer covers the standing-in scene exactly as it will
            // cover the client, and the handover does not restack anything.
            pane_pieces(elements, |elements, piece| match piece {
                Piece::Layers(depth) if state.loading.decorated => {
                    chrome(
                        state,
                        renderer,
                        elements,
                        None,
                        (None, Within::Screen),
                        pane,
                        depth,
                        drawing,
                    );
                }
                Piece::Layers(_) | Piece::Slot(..) => (),
                Piece::Client => {
                    scene(
                        state, renderer, elements, pane, frame, outer.size, now, scale,
                    );
                }
            });
            continue;
        }

        let Some(window) = window else {
            continue;
        };

        // Drawn through its warp, if it has one this pass: the scene fading
        // off it, if any, is in its capture (`offscreen::pane_job`), so the
        // window and what is dissolving off it bend as one.
        if push_warp(
            state,
            prepared,
            elements,
            pane,
            (frame, drawn_global),
            screen,
            scale,
            monitor.as_deref(),
        ) {
            continue;
        }

        // The application has painted and the scene is fading off it. Pushed
        // before the frame and before the client, so it is above both: what is
        // underneath is already the window, and this dissolves to reveal it
        // rather than being swapped for it.
        if state.pane_has_scene(pane) {
            scene(
                state, renderer, elements, pane, frame, outer.size, now, scale,
            );
        }

        let Some(real) = state.real_geometry(&window) else {
            continue;
        };

        // The surface tree is built as if at its real size and then scaled,
        // which keeps subsurface offsets correct for free.
        //
        // **The same arithmetic bridges a live resize**, and that is the point
        // rather than a coincidence: the drawn rectangle and the client's own
        // size agree in ordinary use only because the one is derived from the
        // other. Give the pane a rectangle the client has not agreed to yet --
        // which is what `pane_geometry` does while an edge is being dragged,
        // and what a layout's glide does on every frame between two tiles --
        // and `resize.fill` decides how the buffer the client last painted
        // goes into it, with no second path and nothing new in the element
        // list but the background's strips: at its own size against the edges
        // standing still by default, cut where it is larger and the rest
        // `resize.background` (#233), or stretched to fill under `stretch`.
        // When the client commits the size it was asked for the two sizes are
        // equal again and the window is pixel-exact. See `crate::resizing`
        // and [`fit`].
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
        // Where the buffer's corner goes, which an anchored picture's offset
        // can move off the client's. See [`fit`].
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
        //
        // At the client's alpha, and none once it shows nothing: a menu is
        // the client's (#296).
        let (popups, covered) = popup_elements(
            renderer,
            &window,
            origin,
            output_scale,
            frame.client_alpha(),
        );
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
            |slot| match monitor.as_deref() {
                Some(monitor) => prepared.slots.is_ready_on(&popups_owner, slot, monitor),
                None => prepared.slots.is_ready(&popups_owner, slot),
            },
            |slot| match slot {
                Some(slot) => elements.extend(popups_part.and_then(|part| {
                    slot_element(
                        state,
                        &prepared.slots,
                        &popups_owner,
                        slot,
                        part,
                        scale,
                        drawn_alpha(&frame, Piece::Slot(PaneSlot::Popups, slot))?,
                        (monitor.as_deref(), Within::Screen),
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
                None if !frame.shows_client() => {}
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
        // And the pane's background where the picture does not reach, behind
        // the client: a picture smaller than its rectangle under a fill that
        // does not stretch (#233), cut at the client's corners when the
        // client is. See [`background_strips`].
        let mut background = state
            .panes
            .get(pane)
            .map(|held| {
                background_elements(
                    held,
                    background_strips(
                        state.resizing.background,
                        &placed,
                        real.size,
                        // The pane's background where the client's picture does
                        // not reach: it stands in for the client, so it fades
                        // with it (#296).
                        frame.client_alpha(),
                        scale,
                        rounded.as_ref().map(|(effect, _)| *effect),
                    ),
                    rounded.as_ref().map(|(_, programs)| programs),
                )
            })
            .unwrap_or_default();
        // With its slots: what is ready this pass goes around the pieces it
        // belongs to, and with none the walk is `PANE_ORDER`
        // (`tests::with_no_slots_the_pane_walk_is_pane_order`).
        let ready = slot_ready(&prepared.slots, pane, monitor.as_deref());
        let zoom = frame.rect.size.w / f64::from(outer.size.w.max(1));
        pane_walk(elements, &ready, |elements, piece| match piece {
            Piece::Layers(depth) => chrome(
                state,
                renderer,
                elements,
                Some(&prepared.slots),
                (monitor.as_deref(), Within::Screen),
                pane,
                depth,
                drawing,
            ),
            Piece::Slot(part, slot) => {
                // The client's slots at the client's alpha, and none once it
                // shows nothing; the pane's own at the pane's (#296).
                let Some(alpha) = drawn_alpha(&frame, piece) else {
                    return;
                };
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
                    alpha,
                    (monitor.as_deref(), Within::Screen),
                ));
            }
            Piece::Client => {
                // Popups are not here: they went in above the whole sandwich,
                // before this walk started. See the comment there.

                // A client faded out inside its pane draws nothing, nor the
                // background standing in for it, and its layers are still
                // drawn around the place it was (#296).
                let Some(alpha) = drawn_alpha(&frame, piece) else {
                    return;
                };

                // A surface's top-left is not the window's -- see
                // `window_surface_origin`. That is what made Firefox look
                // both misplaced and shadowed. Popups already did this;
                // toplevels did not.
                let surface_origin = window_surface_origin(origin, &window, scale);
                let window_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    toplevel_elements(renderer, &window, surface_origin, output_scale, alpha);
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
                    // Cut at the same corners: `background_strips` gave them
                    // this clip, taken to the output's pixels.
                    elements.append(&mut background);
                    return;
                }
                // Cut to the tile when there is anything to cut, and a surface
                // the cut leaves nothing of -- a subsurface wholly past the
                // tile's edge -- is dropped: `CropRenderElement` has no empty
                // element to hand back, and says so with `None`.
                elements.extend(window_elements.into_iter().filter_map(|element| {
                    fitted(element, origin, fitting, output_scale).map(Fitted::into_element)
                }));
                // Later in the list is further back: behind the client, in
                // front of the style's `behind` layers, which follow.
                elements.append(&mut background);
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
    raster: crate::surface::Raster,
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
        raster,
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
        crate::surface::Raster::at(scale),
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
    now: std::time::Duration,
) -> Vec<Element> {
    if state.seat.get_pointer().is_none() {
        return Vec::new();
    }
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
    //
    // Where the pointer is *drawn*, which is where it is unless a warp's
    // flight is carrying its picture there (#220), at the instant this
    // screen's frame sampled, so the picture and the windows under it agree.
    // `tests::the_pointer_is_drawn_where_its_flight_has_got_to`.
    let location = state.drawn_pointer_at(now) + shift;

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

/// One window's elements, flat, at the size its pane has, its corner at
/// `at` in the target's physical pixels: as far in as its layers bleed.
///
/// The offscreen pass draws these into a texture so a deformed window is
/// deformed as one thing. Built near the origin because the texture *is*
/// the window's own space; where it lands on screen is the warp's business.
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
/// **A deformed window keeps its bleed** (#312). `offscreen::pane_job` sizes
/// its texture from the window's outer rect grown by its layers' bleed
/// (`render::bled`) and puts the pane's corner at `at`, the bleed's top-left,
/// so a layer placed at `(-bleed.left, -bleed.top)` from the pane lands at
/// the texture's corner; the warp's grid is planned and its mesh drawn over
/// that larger part of the pane's unit square ([`warp_part`]), which maps
/// the texture back onto the window and its shadow where the flat path
/// draws them (`tests::a_bleeding_layer_drawn_through_a_warp_at_rest_lands_where_the_flat_path_draws_it`).
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
///
/// **Its client at `client_opacity`** (#296), the layers opaque: the warp
/// fades the whole texture by the pane's opacity, and only the client fades
/// inside it. `state::tests::real_client::a_warped_panes_capture_is_redrawn_as_its_client_fades`.
pub(crate) fn flat_window_elements(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    window: &Window,
    scale: f64,
    at: Point<i32, Physical>,
    slots: &Slots,
    client_opacity: f32,
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
        at,
        slots,
        &pieces,
        client_opacity,
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
    let pane = state.panes.id_of(window);
    let pieces = pane.map_or_else(|| PANE_ORDER.to_vec(), |pane| inner_pieces(slots, pane));
    // The client as the pane shows it this pass (#296): a whole pane's
    // effect stands for the pane, so the client inside it is already faded,
    // as the warp's capture has it. Read here, where nothing hands a frame
    // in; the pass's instant is the one the walk draws at.
    let client_opacity = pane
        .and_then(|pane| {
            let outer = state.pane_outer_of(pane)?;
            Some(
                state
                    .drawn_id_at(pane, outer, pass_now(state))
                    .client_opacity,
            )
        })
        .unwrap_or(1.0);
    flat_pane(
        state,
        renderer,
        window,
        scale,
        at,
        slots,
        &pieces,
        client_opacity,
    )
}

/// One pane drawn flat at its own size, its corner at `at`, walking
/// `pieces`, its client at `client_opacity` (#296): what both captures of a
/// pane are drawn from.
/// `tests::a_warped_panes_capture_walks_its_client_slot`.
#[expect(
    clippy::too_many_arguments,
    reason = "one pane's capture: whose, at what scale and corner, with what slots, which pieces, and its client's opacity"
)]
fn flat_pane(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    window: &Window,
    scale: f64,
    at: Point<i32, Physical>,
    slots: &Slots,
    pieces: &[Piece],
    client_opacity: f32,
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
    // A T2 slot's result is placed from the pane's own monitor's, where its
    // capture is taken (Ruling F30;
    // `effect::plan::tests::a_monitors_own_result_wins_over_the_slots_one`).
    let monitor = if slots.is_empty() {
        None
    } else {
        pane.and_then(|pane| state.pane_outer_of(pane))
            .and_then(|outer| state.output_of(outer))
            .map(|output| output.name())
    };

    // Fully opaque, and over the whole texture: this pass draws the window flat
    // at its real size and the warp applies the transform's opacity to the
    // whole texture afterwards, so applying it here as well would fade the
    // frame squared. The client within it, but for its own `client_opacity`
    // (#296), which only the client's pixels take: [`piece_alpha`] at a pane
    // opacity of 1. Drawn even at 0, unlike the flat path, which drops it:
    // the capture is redrawn only when its key moves, and a pane with no
    // layers would otherwise have nothing to capture at all.
    let (drawing, origin) = pane_drawing_at(outer, insets, at, scale, pass_now(state));

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
            // Inside a capture, where a slot drawn beneath the pane's warp
            // is its hole (`slot_draw`; Ruling F37).
            Piece::Layers(depth) => {
                if let Some(pane) = pane {
                    chrome(
                        state,
                        renderer,
                        &mut elements,
                        Some(slots),
                        (monitor.as_deref(), Within::Capture),
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
                let alpha = piece_alpha(1.0, client_opacity, piece);
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
                    alpha,
                    (monitor.as_deref(), Within::Capture),
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
                let surfaces = client_piece(
                    renderer,
                    window,
                    surface_origin,
                    output_scale,
                    piece_alpha(1.0, client_opacity, piece),
                );
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
/// where its client's corner goes, the insets inside it. At its layers'
/// bleed, a warp's capture (`offscreen::pane_capture`).
/// `tests::a_pane_drawn_at_the_pad_has_its_client_inside_it`.
pub(crate) fn pane_drawing_at(
    outer: Size<i32, Logical>,
    insets: crate::decoration::Insets,
    at: Point<i32, Physical>,
    scale: f64,
    now: std::time::Duration,
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
        now,
    };
    let client = at
        + Point::<i32, Logical>::from((insets.left, insets.top)).to_physical_precise_round(scale);
    (drawing, client)
}

/// The rectangle a warped window is captured at, and where its client sits in
/// it. See [`flat`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Flat {
    /// The pane's outer size, in logical pixels: the texture's, grown by
    /// its layers' bleed (`offscreen::pane_capture`).
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
/// builds the warp's mesh over (grown on both by its layers' bleed,
/// [`warp_part`]), so the texture and the mesh are one size.
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
        state.resize_pins(pane.id()),
    );
    Placed {
        client,
        origin: client.loc + fitting.offset,
        fit: fitting,
    }
}

/// How a client's surfaces go into the rectangle they are drawn in: the
/// factor every surface is scaled by, about the client's corner, where that
/// corner is, and the rectangle they are cut to when there is one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fit {
    /// What the whole committed buffer is scaled by. Also what the popups are
    /// scaled by, which is why it is separate from the cut: they take this and
    /// never the other.
    pub(crate) factor: Scale<f64>,
    /// Where the buffer's top-left corner is drawn, from the drawn client
    /// rectangle's, in drawn logical pixels: nothing, except where
    /// [`crate::resizing::Fill::Anchor`] keeps a picture against a far edge
    /// that is standing still. Then it is the drawn width (or height) less the
    /// picture's, which is the gap a smaller picture leaves on the moving side,
    /// or, negative, the part of a larger picture that is cut away past the
    /// moving side. `tests::an_anchored_picture_stays_against_the_edge_that_does_not_move`.
    pub(crate) offset: Point<f64, Logical>,
    /// The drawn client rectangle, for a client whose picture is cut to it.
    /// `None` for every other client, which is drawn through no crop at all,
    /// so a rounding in this rectangle cannot shave a pixel off a window that
    /// had nothing to cut. `fitted` rounds it to the output's pixels.
    pub(crate) crop: Option<Rectangle<f64, Logical>>,
    /// How much of the committed buffer is in the picture: all of it, except
    /// along an axis it is cut on. From its top-left corner unless `offset`
    /// moves the picture, which only an anchored fill does.
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
/// `tiled` is whether a layout holds the pane in a tile, `fill` how the
/// picture goes into a rectangle it was not drawn at (`resize.fill`,
/// [`crate::resizing::Fill`]), and `pins` which of its left and top edges are
/// moving with the edge opposite standing still (`Solium::resize_pins`).
///
/// **Under every fill but `stretch` the picture is never scaled** (#233):
/// each axis is drawn at the size the client drew it, cut to the pictured
/// rectangle where it is larger, and short of it where it is smaller, the
/// rest filled by the pane's background ([`uncovered`]). `anchor` keeps it
/// against the edges `pins` says are standing still, `crop` at the top-left.
/// Then the whole of it is scaled by `zoom`, and the cut, when there is one,
/// is `client` itself. So, with `crate::resizing::Fill::Anchor`:
///
/// * A glide that shrinks a window whose client has answered draws the
///   answer at its own size and leaves the rest of the rectangle to the
///   background, rather than stretching it up to 1.8x:
///   `a_glide_that_shrinks_a_window_draws_its_answer_at_its_own_size`.
/// * One that grows a window before its client has answered does the same
///   with the old picture, and one whose client answered first cuts the
///   larger picture: `a_grow_is_not_stretched_either_way`.
/// * A one-pixel buffer is a one-pixel strip, not a pane (#125):
///   `a_one_pixel_buffer_is_not_stretched_across_its_pane`.
///
/// **`stretch` is what this did before #233, kept exactly.** Per axis, the buffer is drawn at its own size where the pictured
/// rectangle is narrower than it and the pane is tiled -- the tile cuts it --
/// and is scaled to fill the pictured rectangle everywhere else:
///
/// * A settled client that committed more than its tile is shown 1:1, cut to
///   the tile, and not squashed into it (under every fill):
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
///   buffer into it, and one that changes the two axes by different amounts
///   does each axis on its own:
///   `a_glide_between_tiles_of_different_shapes_is_fitted_per_axis`.
pub(crate) fn fit(
    client: Rectangle<f64, Logical>,
    zoom: (f64, f64),
    committed: Size<i32, Logical>,
    tiled: bool,
    fill: crate::resizing::Fill,
    pins: (bool, bool),
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
    // A stretch cuts only what a tile holds (#133) and scales the rest; every
    // other fill cuts whatever is larger than the picture, tiled or not, since
    // it never scales.
    // `tests::fitting::a_picture_larger_than_its_rectangle_is_cut_and_never_squashed`.
    let cuts = |pictured: f64, committed: i32| {
        (tiled || !fill.stretches()) && pictured < f64::from(committed) - CUT
    };
    let (cut_x, cut_y) = (cuts(pictured.w, committed.w), cuts(pictured.h, committed.h));
    let (across, down) = crate::resizing::factor(fill, pictured, committed);
    let factor = Scale::from((
        zoom.0 * if cut_x { 1.0 } else { across },
        zoom.1 * if cut_y { 1.0 } else { down },
    ));
    // An anchored picture's far edge on the rectangle's, on an axis whose near
    // edge is the one moving: the drawn size less the picture's, which is the
    // gap left on the moving side or, negative, what is cut away past it.
    // Nothing for any other fill, and nothing on an axis that is not pinned.
    // `tests::fitting::an_anchored_picture_stays_against_the_edge_that_does_not_move`.
    let offset = |pinned: bool, drawn: f64, committed: i32, factor: f64| {
        if pinned && fill.anchors() {
            drawn - f64::from(committed) * factor
        } else {
            0.0
        }
    };
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
        offset: (
            offset(pins.0, client.size.w, committed.w, factor.x),
            offset(pins.1, client.size.h, committed.h, factor.y),
        )
            .into(),
        crop: (cut_x || cut_y).then_some(client),
        shown: (
            kept(cut_x, pictured.w, committed.w),
            kept(cut_y, pictured.h, committed.h),
        )
            .into(),
    }
}

/// The parts of a client's drawn rectangle its picture does not cover, in the
/// output's pixels: what the pane's background (`resize.background`) fills
/// under a fill that does not stretch (#233).
///
/// `client` is the drawn client rectangle, `origin` where the picture's corner
/// is drawn and `picture` its drawn size -- the committed size times the fit's
/// factor. Four strips, in this order: the band above the picture and the band
/// below it, each the whole width, and the strips left and right of it between
/// them. Each is empty where the picture reaches that side, and all are when
/// it covers the rectangle, which is every stretched picture and every picture
/// at rest.
///
/// **The rectangle is rounded as [`fitted`] rounds a cut, and the picture as
/// smithay rounds a surface** -- its corner and its size each to the nearest
/// pixel -- to decide which sides it reaches, so a picture that covers its
/// rectangle leaves no strip at any output scale, nor does one a rounding
/// short of it: a window at rest draws nothing extra.
/// `tests::the_background_fills_what_the_picture_leaves_uncovered`.
///
/// **On a side it does not reach, the strip ends under the picture's edge,
/// rounded inward**, by up to a pixel, rather than stopping a pixel short of
/// it: the background is drawn behind the client, so an overlap is hidden by
/// it, and a gap would be a line of whatever is behind the window. Each
/// strip is clamped to the rectangle, so a picture less than a pixel tall
/// makes no strip of negative size (#125's `524x1`, zoomed).
pub(crate) fn uncovered(
    client: Rectangle<f64, Logical>,
    origin: Point<f64, Logical>,
    picture: Size<f64, Logical>,
    scale: f64,
) -> [Rectangle<i32, Physical>; 4] {
    let room = Rectangle::new(
        client.loc.to_physical_precise_round(scale),
        client.size.to_physical_precise_round(scale),
    );
    let drawn: Rectangle<i32, Physical> = Rectangle::new(
        origin.to_physical_precise_round(scale),
        picture.to_physical_precise_round(scale),
    );
    let nothing = Rectangle::new(room.loc, Size::default());
    let Some(covered) = room.intersection(drawn) else {
        return [room, nothing, nothing, nothing];
    };
    let (left, top) = (room.loc.x, room.loc.y);
    let (right, bottom) = (left + room.size.w, top + room.size.h);
    let exact = Rectangle::new(origin, picture).to_physical(scale);
    let near = exact.loc.to_i32_ceil::<i32>();
    let far = (exact.loc + exact.size).to_i32_floor::<i32>();
    let (inner_left, inner_top) = (
        if covered.loc.x > left {
            near.x.clamp(left, right)
        } else {
            left
        },
        if covered.loc.y > top {
            near.y.clamp(top, bottom)
        } else {
            top
        },
    );
    let (inner_right, inner_bottom) = (
        if covered.loc.x + covered.size.w < right {
            far.x.clamp(inner_left, right)
        } else {
            right
        },
        if covered.loc.y + covered.size.h < bottom {
            far.y.clamp(inner_top, bottom)
        } else {
            bottom
        },
    );
    [
        Rectangle::from_extremities((left, top), (right, inner_top)),
        Rectangle::from_extremities((left, inner_bottom), (right, bottom)),
        Rectangle::from_extremities((left, inner_top), (inner_left, inner_bottom)),
        Rectangle::from_extremities((inner_right, inner_top), (right, inner_bottom)),
    ]
}

/// `resize.background` as the premultiplied colour a strip of it is drawn in,
/// at a frame's `opacity`; `None` for `"none"`, which draws no strip at all.
/// `tests::the_background_is_drawn_in_its_configured_colour`.
pub(crate) fn background_colour(
    background: crate::resizing::Background,
    opacity: f32,
) -> Option<[f32; 4]> {
    let [red, green, blue, alpha] = background?;
    let alpha = f32::from(alpha) / 255.0 * opacity;
    let channel = |value: u8| f32::from(value) / 255.0 * alpha;
    Some([channel(red), channel(green), channel(blue), alpha])
}

/// A pane's background strips, before they are elements (#233).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Strips {
    /// [`uncovered`]'s four, in the output's pixels: empty where the picture
    /// reaches that side.
    pub(crate) rects: [Rectangle<i32, Physical>; 4],
    /// [`background_colour`]'s, the frame's opacity in it.
    pub(crate) colour: [f32; 4],
    /// The rounded rectangle the client is drawn through, in the output's
    /// pixels, when it is: the strips are drawn through it too.
    pub(crate) clip: Option<crate::clip::Clip>,
}

/// The strips of a pane's background round a picture that does not cover its
/// rectangle: where they are, their colour, and their clip. `None` when the
/// background is `"none"`.
///
/// **Cut at the client's corners when the client is.** `rounding` is the
/// one `panes` draws the client through (a style's `client.radius`); the
/// strips take the same rounded rectangle, the client's as drawn, with a
/// `factor` of 1 because a strip is drawn in the output's pixels and never
/// rescaled. Drawn square, a strip stood past a rounded window's corners in
/// the background's colour for every frame of a glide or a drag, and covered
/// what the style draws behind the cut corners (`rounded`'s bar).
/// `tests::fitting::a_rounded_clients_background_is_cut_at_its_corners`.
pub(crate) fn background_strips(
    background: crate::resizing::Background,
    placed: &Placed,
    committed: Size<i32, Logical>,
    opacity: f32,
    scale: f64,
    rounding: Option<solium_effects::fragment::Effect>,
) -> Option<Strips> {
    let colour = background_colour(background, opacity)?;
    let picture = Size::from((
        f64::from(committed.w) * placed.fit.factor.x,
        f64::from(committed.h) * placed.fit.factor.y,
    ));
    let clip = rounding.map(|effect| {
        drawn_clip(
            Rectangle::new(
                placed.client.loc.to_physical_precise_round(scale),
                placed.client.size.to_physical_precise_round(scale),
            ),
            Point::default(),
            Scale::from((1.0, 1.0)),
            crate::pass::physical_radii(effect, scale),
        )
    });
    Some(Strips {
        rects: uncovered(placed.client, placed.origin, picture, scale),
        colour,
        clip,
    })
}

/// [`background_strips`]' strips as elements, each under its own identity
/// from `Pane::background`, the empty ones left out: plain solids, or solids
/// through the client's clip when there is one and `programs`, the clip's
/// compiled pair, to draw it with -- without them a rounded client is drawn
/// square, and so are its strips. Nothing for no strips (`"none"`).
/// `tests::fitting::the_background_is_drawn_where_the_picture_does_not_reach`.
pub(crate) fn background_elements(
    pane: &Pane,
    strips: Option<Strips>,
    programs: Option<&crate::pass::ClipPrograms>,
) -> Vec<Element> {
    let Some(strips) = strips else {
        return Vec::new();
    };
    strips
        .rects
        .into_iter()
        .zip(pane.background())
        .filter(|(strip, _)| strip.size.w > 0 && strip.size.h > 0)
        .map(|(strip, id)| {
            let solid = smithay::backend::renderer::element::solid::SolidColorRenderElement::new(
                id.clone(),
                strip,
                CommitCounter::default(),
                strips.colour,
                Kind::Unspecified,
            );
            match (strips.clip, programs) {
                (Some(clip), Some(programs)) => Element::ClippedSolid(
                    crate::clip::ClippedSolid::new(solid, clip, programs.clone()),
                ),
                _ => Element::Solid(solid),
            }
        })
        .collect()
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
        // What the client left fades with `client_opacity` as the client did
        // (#296), and the fill below with it.
        return picture
            .elements(surfaces, scale, frame.client_alpha())
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
    let alpha = fill[3] * frame.client_alpha();
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
/// popups, at `alpha`: the client's opacity inside its pane in a warp's
/// capture (#296), 1 in the client's own self capture, whose result is
/// faded where it is placed.
/// `state::tests::a_warped_panes_capture_holds_no_popups`.
pub(crate) fn client_piece<R>(
    renderer: &mut R,
    window: &Window,
    origin: Point<i32, Physical>,
    scale: Scale<f64>,
    alpha: f32,
) -> Vec<WaylandSurfaceRenderElement<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    toplevel_elements(renderer, window, origin, scale, alpha)
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
    use super::{
        Drawn, Fit, Fitted, Painted, background_colour, by_depth, fit, fitted, origin_at, ratio,
        uncovered,
    };
    use crate::qml::qt_test::on_the_qt_thread;

    /// A frame for a `w` x `h` pane, presented at `drawn`.
    fn presented_at(w: i32, h: i32, drawn: (f64, f64)) -> crate::present::Frame {
        let mut frame =
            crate::present::Frame::real(smithay::utils::Rectangle::from_size((w, h).into()));
        frame.rect.size = drawn.into();
        frame
    }

    /// **A pane drawn at a quarter is captured at a quarter** of its full
    /// resolution (#307), and whole again once drawn whole; a size between two
    /// steps takes the larger, so nothing it shows is drawn from fewer pixels
    /// than the screen has for it.
    #[test]
    fn a_pane_drawn_at_a_quarter_is_captured_at_a_quarter() {
        let outer = smithay::utils::Size::from((1600, 900));
        let fraction =
            |drawn| super::capture_fraction(&presented_at(1600, 900, drawn), None, outer);
        assert_eq!(fraction((400.0, 225.0)), 0.25);
        assert_eq!(fraction((1600.0, 900.0)), 1.0);
        assert_eq!(fraction((410.0, 120.0)), 0.375, "the larger step");
        assert_eq!(fraction((20.0, 20.0)), 0.125, "never less than an eighth");
        assert_eq!(
            super::whole_size(smithay::utils::Size::from((400, 225)), 0.25),
            smithay::utils::Size::from((1600, 900)),
            "a present's pixels are told the size the capture stands for"
        );
    }

    /// **A grid that keeps one cell whole is captured whole**: a mesh is
    /// measured cell by cell, so a part it stretches keeps its pixels even
    /// when the rest of the pane is drawn small.
    #[test]
    fn a_grid_that_keeps_one_cell_whole_is_captured_whole() {
        let outer = smithay::utils::Size::from((1600, 900));
        let frame = presented_at(1600, 900, (400.0, 225.0));
        // Two cells by one over the whole pane: the left squeezed to a
        // sliver, the right `right` wide against the 800 its half spans.
        let grid = |right: f64| crate::warp::Grid {
            cols: 2,
            rows: 1,
            part: crate::warp::UnitRect::WHOLE,
            points: vec![
                0.0,
                0.0,
                10.0,
                0.0,
                10.0 + right,
                0.0,
                0.0,
                225.0,
                10.0,
                225.0,
                10.0 + right,
                225.0,
            ],
        };
        assert_eq!(
            super::capture_fraction(&frame, Some(&grid(800.0)), outer),
            1.0
        );
        assert_eq!(
            super::capture_fraction(&frame, Some(&grid(200.0)), outer),
            0.25
        );
    }

    /// **A tilted pane is captured whole**: a perspective may show any part
    /// of it larger than its rectangle says.
    #[test]
    fn a_tilted_pane_is_captured_whole() {
        let mut frame = presented_at(1600, 900, (400.0, 225.0));
        frame.matrix = crate::mat4::Mat4::rotate_y(0.3);
        assert_eq!(
            super::capture_fraction(&frame, None, smithay::utils::Size::from((1600, 900))),
            1.0
        );
    }

    /// **A grid over the bleed is measured across its part** (#312): a
    /// pane's grid now spans its layers' bleed as well, and a cell is still
    /// held to what it spans of the pane's own pixels, so a pane drawn at a
    /// quarter is captured at a quarter, bleed and all.
    #[test]
    fn a_grid_over_the_bleed_is_measured_across_its_part() {
        let outer = smithay::utils::Size::from((1600, 900));
        let frame = presented_at(1600, 900, (400.0, 225.0));
        // 160 of bleed either side: the part 1920 of the pane's pixels wide,
        // drawn 480 wide.
        let part = super::pane_part(
            outer,
            crate::style::Bleed {
                left: 160,
                right: 160,
                ..crate::style::Bleed::default()
            },
        );
        let grid = crate::warp::Grid {
            cols: 1,
            rows: 1,
            part,
            points: vec![-40.0, 0.0, 440.0, 0.0, -40.0, 225.0, 440.0, 225.0],
        };
        assert_eq!(super::capture_fraction(&frame, Some(&grid), outer), 0.25);
    }

    /// A 300 by 200 pane's style bleeding 12 above it, 6 to its right, 4
    /// below and 20 to its left: a shadow, and a title above it.
    fn bleeding() -> crate::style::Bleed {
        crate::style::Bleed {
            top: 12,
            right: 6,
            bottom: 4,
            left: 20,
        }
    }

    /// **A warped pane's part reaches past it by its bleed** (#312): its
    /// capture holds the pane grown by its layers' bleed, the pane's corner
    /// as far in as the bleed at every scale, a fractional one rounded as a
    /// capture is; and its part of the pane's unit square reaches past
    /// [0, 1] by as much, as its popups' does past the pane.
    #[test]
    fn a_warped_panes_part_reaches_past_it_by_its_bleed() {
        use smithay::utils::{Rectangle, Size};
        let outer = Size::from((300, 200));
        assert_eq!(
            super::pane_part(outer, bleeding()),
            crate::warp::UnitRect {
                u0: -20.0 / 300.0,
                v0: -12.0 / 200.0,
                u1: 1.0 + 6.0 / 300.0,
                v1: 1.0 + 4.0 / 200.0,
            }
        );
        assert_eq!(
            super::bled(Rectangle::new((100, 50).into(), outer), bleeding()),
            Rectangle::new((80, 38).into(), (326, 216).into())
        );
        for (scale, size, corner) in [
            (1.0, (326, 216), (20, 12)),
            (2.0, (652, 432), (40, 24)),
            (1.25, (408, 270), (25, 15)),
        ] {
            assert_eq!(
                crate::offscreen::pane_capture(outer, bleeding(), scale),
                (size.into(), corner.into()),
                "captured at {scale}"
            );
        }
    }

    /// **A pane that bleeds nothing is captured as it always was**: at its
    /// outer rectangle's size, its corner at the origin, over the whole
    /// pane, its pixels' subject the whole capture.
    #[test]
    fn a_pane_that_bleeds_nothing_is_captured_as_it_always_was() {
        let outer = smithay::utils::Size::from((300, 200));
        let none = crate::style::Bleed::default();
        assert_eq!(super::pane_part(outer, none), crate::warp::UnitRect::WHOLE);
        for (scale, size) in [(1.0, (300, 200)), (1.25, (375, 250)), (1.5, (450, 300))] {
            assert_eq!(
                crate::offscreen::pane_capture(outer, none, scale),
                (size.into(), (0, 0).into()),
                "captured at {scale}"
            );
        }
        assert_eq!(
            super::content_in(crate::warp::UnitRect::WHOLE),
            [0.0, 0.0, 1.0, 1.0]
        );
    }

    /// **`sol_content` names the pane inside its capture** (#312): its
    /// corner and its size in the `uv` of a capture that holds its layers'
    /// bleed, so a frag finds the pane and not the shadow around it.
    #[test]
    fn sol_content_names_the_pane_inside_its_capture() {
        let content = super::content_in(super::pane_part(
            smithay::utils::Size::from((300, 200)),
            bleeding(),
        ));
        let wanted = [20.0 / 326.0, 12.0 / 216.0, 300.0 / 326.0, 200.0 / 216.0];
        for (got, want) in content.into_iter().zip(wanted) {
            assert!(
                (f64::from(got) - want).abs() < 1e-6,
                "the pane is at {content:?} in its capture, not {wanted:?}"
            );
        }
    }

    /// **A bleeding layer drawn through a warp at rest lands where the flat
    /// path draws it** (#312): the layer's canvas, drawn into the capture
    /// from the pane's corner at the bleed, and carried onto the screen by
    /// the mesh over the pane's part with nothing moving it, is where
    /// `elements` puts it standing still, and so is the pane, to within the
    /// capture's rounding at a fractional scale.
    #[test]
    fn a_bleeding_layer_drawn_through_a_warp_at_rest_lands_where_the_flat_path_draws_it() {
        use smithay::utils::{Logical, Rectangle, Size};
        let outer = Size::<i32, Logical>::from((300, 200));
        let insets = crate::decoration::Insets {
            top: 30,
            ..crate::decoration::Insets::default()
        };
        let rest = Rectangle::<f64, Logical>::new((100.0, 50.0).into(), (300.0, 200.0).into());
        let now = std::time::Duration::ZERO;
        for scale in [1.0, 2.0, 1.25] {
            let (size, corner) = crate::offscreen::pane_capture(outer, bleeding(), scale);
            let (drawing, _) = super::pane_drawing_at(outer, insets, corner, scale, now);
            let captured = crate::decoration::spread(drawing, bleeding()).drawn;
            let mesh = crate::warp::mesh_part(
                rest,
                super::pane_part(outer, bleeding()),
                crate::mat4::Mat4::IDENTITY,
                (0.5, 0.5),
                scale,
            )
            .expect("a mesh");
            // One cell: its first vertex is the texture's (0, 0), its third
            // the texture's (1, 1).
            let (first, third) = (mesh.vertices()[0], mesh.vertices()[2]);
            // Where a point drawn into the capture lands on the screen, and
            // whether the capture holds it at all: a layer drawn off its
            // edge is cut there, wherever the mesh would have put it.
            let on_screen = |x: f64, y: f64| {
                let (u, v) = (x * scale / f64::from(size.w), y * scale / f64::from(size.h));
                let held = (-1e-9..=1.0 + 1e-9).contains(&u) && (-1e-9..=1.0 + 1e-9).contains(&v);
                (
                    f64::from(first.x) + u * f64::from(third.x - first.x),
                    f64::from(first.y) + v * f64::from(third.y - first.y),
                    held,
                )
            };
            let flat = crate::decoration::spread(
                crate::decoration::Drawing {
                    rect: rest,
                    outer,
                    alpha: 1.0,
                    scale,
                    now,
                },
                bleeding(),
            )
            .drawn;
            let far = |rect: Rectangle<f64, Logical>| {
                (rect.loc.x + rect.size.w, rect.loc.y + rect.size.h)
            };
            for (what, (x, y), (want_x, want_y)) in [
                (
                    "the layer's corner",
                    (captured.loc.x, captured.loc.y),
                    (flat.loc.x, flat.loc.y),
                ),
                ("the layer's far corner", far(captured), far(flat)),
                (
                    "the pane's corner",
                    (drawing.rect.loc.x, drawing.rect.loc.y),
                    (rest.loc.x, rest.loc.y),
                ),
            ] {
                let (got_x, got_y, held) = on_screen(x, y);
                assert!(
                    held,
                    "at {scale} {what} is drawn off the capture, at ({x}, {y})"
                );
                assert!(
                    (got_x - want_x * scale).abs() <= 1.0 && (got_y - want_y * scale).abs() <= 1.0,
                    "at {scale} {what} lands at ({got_x}, {got_y}), not where the flat path \
                     draws it, ({}, {})",
                    want_x * scale,
                    want_y * scale
                );
            }
        }
    }

    /// **A hole is masked at its place in a capture that bleeds** (#312):
    /// a slot drawn beneath a warped pane is cut by its part's shape where
    /// the part is in the capture, the pane's corner at the bleed: the
    /// client's corner in the mask is the client's corner in the capture.
    #[test]
    fn a_hole_is_masked_at_its_place_in_a_capture_that_bleeds() {
        use smithay::utils::{Logical, Rectangle};
        let outer = Rectangle::<i32, Logical>::new((100, 50).into(), (300, 200).into());
        let client = Rectangle::<i32, Logical>::new((100, 80).into(), (300, 170).into());
        let scale = 2.0;
        let mask = crate::effect::beneath::beneath_mask(
            client,
            super::bled(outer, bleeding()),
            [0.0; 4],
            scale,
        );
        let (size, corner) = crate::offscreen::pane_capture(outer.size, bleeding(), scale);
        // The client 30 below the pane's corner, in the capture's pixels.
        let in_capture = (f64::from(corner.x), f64::from(corner.y) + 30.0 * scale);
        assert_eq!(
            (
                f64::from(mask.rect[0]) / f64::from(mask.size[0]),
                f64::from(mask.rect[1]) / f64::from(mask.size[1]),
            ),
            (
                in_capture.0 / f64::from(size.w),
                in_capture.1 / f64::from(size.h),
            )
        );
    }

    /// **A part's beneath bounds on a grid over the bleed are its own
    /// cells** (#312): a pane's grid runs across its layers' bleed too, and
    /// the pane's own part of it is found across that, not across the grid.
    #[test]
    fn a_parts_beneath_bounds_on_a_grid_over_the_bleed_are_its_own_cells() {
        let frame = crate::present::Frame::real(smithay::utils::Rectangle::new(
            (0, 0).into(),
            (100, 100).into(),
        ));
        // 50 of bleed either side, four cells across, each 50 wide.
        let part = super::pane_part(
            smithay::utils::Size::from((100, 100)),
            crate::style::Bleed {
                left: 50,
                right: 50,
                ..crate::style::Bleed::default()
            },
        );
        let mut points = Vec::new();
        for y in [0.0, 100.0] {
            for x in [-50.0, 0.0, 50.0, 100.0, 150.0] {
                points.extend_from_slice(&[x, y]);
            }
        }
        let grid = crate::warp::Grid {
            cols: 4,
            rows: 1,
            part,
            points,
        };
        let bounds = super::beneath_bounds(&frame, Some(&grid), crate::warp::UnitRect::WHOLE)
            .expect("bounds");
        assert_eq!(
            (bounds.loc.x, bounds.size.w),
            (0.0, 100.0),
            "the pane's bounds are {bounds:?}, its shadow's cells and all"
        );
    }

    /// **A standing-in scene is laid out at its window's own size**: the
    /// shipped loading scene, told how long it has waited on every frame as
    /// `render::scene` tells it, is drawn once through a window zooming open
    /// from 0.88 and once more through a glide that takes the window to a new
    /// size, and not at all through a fade. Laid out at the presented
    /// rectangle, as it was, every frame of the zoom and of the glide was a
    /// new size and a new picture (on the GPU, a new buffer).
    #[test]
    fn a_standing_in_scene_is_laid_out_at_its_windows_own_size() {
        use smithay::utils::{Logical, Rectangle, Size};
        on_the_qt_thread(|| {
            if crate::qml::on_gpu() {
                return;
            }
            let mut scene = crate::surface::ShellSurface::new(
                crate::pane::loading_source(None),
                r#"{"program":"kitty","waited":0}"#,
            )
            .expect("the shipped loading scene builds");
            let raster = crate::surface::Raster::at(1.0);
            let mut drawn = 0;
            let mut waited = 0;
            // One frame of `render::scene`'s: whether it drew a new picture.
            let mut frame = |scene: &mut crate::surface::ShellSurface,
                             outer: Size<i32, Logical>,
                             rect: Rectangle<f64, Logical>,
                             opacity: f32| {
                let mut at = crate::present::Frame::real(Rectangle::from_size(outer));
                at.rect = rect;
                at.opacity = opacity;
                let (laid_out, placed) = super::standing_layout(outer, &at);
                assert_eq!(placed, rect, "the picture is drawn where the frame is");
                waited += 16;
                scene.set_int("waited", waited);
                scene.lay_out(raster.pixels(laid_out), 1.0);
                let qt = scene.scene_for_test();
                let fresh = qt.needs_render();
                if fresh {
                    let _ = qt.render();
                }
                u32::from(fresh)
            };
            let outer: Size<i32, Logical> = (400, 300).into();
            // Zooming open about its centre, 0.88 to 1, as `open.lua` does.
            for step in 0..=30 {
                let zoom = 0.88 + 0.12 * f64::from(step) / 30.0;
                let (w, h) = (400.0 * zoom, 300.0 * zoom);
                let rect = Rectangle::new(
                    (100.0 + (400.0 - w) / 2.0, 80.0 + (300.0 - h) / 2.0).into(),
                    (w, h).into(),
                );
                drawn += frame(&mut scene, outer, rect, 1.0);
            }
            assert_eq!(drawn, 1, "drawn through the zoom");
            // Its tile becomes 600 by 300: the pane has its new size at once,
            // and the frame glides there from the old rectangle.
            let wider: Size<i32, Logical> = (600, 300).into();
            for step in 0..=30 {
                let t = f64::from(step) / 30.0;
                let rect = Rectangle::new(
                    (100.0 - 100.0 * t, 80.0).into(),
                    (400.0 + 200.0 * t, 300.0).into(),
                );
                drawn += frame(&mut scene, wider, rect, 1.0);
            }
            assert_eq!(drawn, 2, "drawn through the glide to a new size");
            // And fading as its application arrives.
            let rest = Rectangle::new((0.0, 80.0).into(), (600.0, 300.0).into());
            for step in 0_u8..=10 {
                drawn += frame(&mut scene, wider, rest, 1.0 - f32::from(step) / 10.0);
            }
            assert_eq!(drawn, 2, "drawn through the fade");
        });
    }

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
        let (mut memos, xrays) = (
            crate::effect::memo::Memos::default(),
            crate::effect::xray::Xrays::default(),
        );
        let mut cx = super::RunCx {
            store: &mut store,
            effects: &host,
            chains: &mut chains,
            masked: None,
            now: 0.0,
            pass: 1,
            memos: &mut memos,
            xrays: &xrays,
            motion: (std::time::Duration::ZERO, 0.0),
            waited: false,
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
        let (mut memos, xrays) = (
            crate::effect::memo::Memos::default(),
            crate::effect::xray::Xrays::default(),
        );
        let mut cx = super::RunCx {
            store: &mut store,
            effects: &host,
            chains: &mut chains,
            masked: None,
            now: 0.0,
            pass: 1,
            memos: &mut memos,
            xrays: &xrays,
            motion: (std::time::Duration::ZERO, 0.0),
            waited: false,
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
        use super::{Fit, Fitted, background_colour, fit, fitted, uncovered};
        use crate::resizing::Fill;
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

        /// What every fit was before #233, and what `resize.fill = "stretch"`
        /// still is.
        const STRETCH: crate::resizing::Fill = crate::resizing::Fill::Stretch;

        /// No left or top edge moving with the edge opposite standing still:
        /// a pane at rest, or a change that moves a right or bottom edge, or
        /// both of an axis' edges.
        const STILL: (bool, bool) = (false, false);

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

            for fill in [STRETCH, Fill::Anchor, Fill::Crop, Fill::Scene] {
                let settled = fit(client, AT_REST, committed, true, fill, STILL);
                assert_eq!(settled.factor, Scale::from((1.0, 1.0)), "{fill:?}");
                assert_eq!(settled.crop, Some(client), "{fill:?}");
            }
            let settled = fit(client, AT_REST, committed, true, STRETCH, STILL);
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
                    STRETCH,
                    STILL,
                );
                assert_eq!(
                    settled,
                    Fit {
                        factor: Scale::from((1.0, 1.0)),
                        offset: Point::default(),
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
                STRETCH,
                STILL,
            );
            assert_eq!(
                held,
                Fit {
                    factor: Scale::from((300.0 / 64.0, 200.0 / 64.0)),
                    offset: Point::default(),
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
                STRETCH,
                STILL,
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
                STRETCH,
                STILL,
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
                STRETCH,
                STILL,
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
                STRETCH,
                STILL,
            );
            assert_eq!(
                first,
                Fit {
                    factor: Scale::from((1.0, 1.0)),
                    offset: Point::default(),
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
                STRETCH,
                STILL,
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
                STRETCH,
                STILL,
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
                STRETCH,
                STILL,
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

            let unanswered = fit(
                client,
                AT_REST,
                Size::from((1900, 1000)),
                true,
                STRETCH,
                STILL,
            );
            assert_eq!(unanswered.factor, Scale::from((1.0, 1.0)));
            assert_eq!(unanswered.crop, Some(client));
            assert_eq!(unanswered.shown, Size::from((1400, 1000)));

            let answered = fit(
                client,
                AT_REST,
                Size::from((950, 1000)),
                true,
                STRETCH,
                STILL,
            );
            assert_eq!(
                answered.factor,
                Scale::from((1400.0 / 950.0, 1.0)),
                "stretched across and not down"
            );
            assert_eq!(answered.crop, None, "and nothing to cut");

            let mixed = fit(
                client,
                AT_REST,
                Size::from((1900, 800)),
                true,
                STRETCH,
                STILL,
            );
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
                STRETCH,
                STILL,
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

        /// A buffer rectangle that does not start at the buffer's corner.
        fn part(x: f64, y: f64, w: f64, h: f64) -> Rectangle<f64, Buffer> {
            Rectangle::new((x, y).into(), (w, h).into())
        }

        /// The fills that never scale a picture (#233).
        const UNSCALED: [Fill; 3] = [Fill::Anchor, Fill::Crop, Fill::Scene];

        /// **#233: each fill fits a picture its rectangle has outgrown.**
        ///
        /// The glide #233 is about: a terminal 1900 wide halved to 950 for a
        /// new neighbour, whose client answered 950 within a frame or two,
        /// drawn while the glide is still 1400 wide. `stretch` -- every glide
        /// before #233 -- draws the 950 across 1400, glyphs 1.47x wide; the
        /// others draw it at its own size and leave 450 to the background.
        /// Nothing is cut either way: the picture is inside the rectangle.
        #[test]
        fn each_fill_fits_a_picture_its_rectangle_has_outgrown() {
            let client = logical(0.0, 0.0, 1400.0, 1000.0);
            let answered = Size::from((950, 1000));
            let stretched = fit(client, AT_REST, answered, true, STRETCH, STILL);
            assert_eq!(
                stretched.factor,
                Scale::from((1400.0 / 950.0, 1.0)),
                "a stretch scales the answer into the glide"
            );
            for fill in UNSCALED {
                let fitted = fit(client, AT_REST, answered, true, fill, STILL);
                assert_eq!(
                    fitted,
                    Fit {
                        factor: Scale::from((1.0, 1.0)),
                        offset: Point::default(),
                        crop: None,
                        shown: answered,
                    },
                    "{fill:?} draws the answer at its own size, from the corner"
                );
            }
            assert_eq!(
                fit(client, (0.5, 0.5), answered, true, Fill::Anchor, STILL).factor,
                Scale::from((0.5, 0.5)),
                "and a frame's zoom still scales the whole window"
            );
        }

        /// **A frame's zoom enlarges a window under every fill.** An
        /// overview thumbnail scaled up, or a mode that enlarges, pictures
        /// the window at the size it committed and scales the whole of it by
        /// the zoom, so no fill has anything to decide there. Before #133 the
        /// enlargement came through the fill's own factor, and `resizing`
        /// pinned that no setting could clamp it
        /// (`a_window_that_is_not_being_dragged_is_never_clamped`, gone with
        /// that arithmetic).
        #[test]
        fn a_frames_zoom_enlarges_a_window_under_every_fill() {
            for fill in [STRETCH, Fill::Anchor, Fill::Crop, Fill::Scene] {
                let enlarged = fit(
                    logical(0.0, 0.0, 1600.0, 1200.0),
                    (2.0, 2.0),
                    Size::from((800, 600)),
                    false,
                    fill,
                    STILL,
                );
                assert_eq!(enlarged.factor, Scale::from((2.0, 2.0)), "{fill:?}");
                assert_eq!(enlarged.crop, None, "{fill:?}");
                assert_eq!(enlarged.offset, Point::default(), "{fill:?}");
            }
        }

        /// **#233: an anchored picture stays against the edge that does not
        /// move**, for a shrink from each of the four sides, before its
        /// client answers -- the old picture larger than the rectangle, cut
        /// on the moving side -- and after -- the answer smaller than the
        /// rectangle the glide or drag has reached, the gap on the moving
        /// side. `pins` is which of the left and top edges moves alone.
        /// `crop` keeps the corner whatever moves, and `stretch` has nothing
        /// to anchor.
        #[test]
        fn an_anchored_picture_stays_against_the_edge_that_does_not_move() {
            // Shrinking from 800x500 towards 400x300, drawn at 600x400 at
            // (100, 50) part of the way there.
            let client = logical(100.0, 50.0, 600.0, 400.0);
            let before = Size::from((800, 500));
            let after = Size::from((400, 300));
            // (pins, offset before the answer, offset after it)
            for (side, pins, larger, smaller) in [
                ("left", (true, false), (-200.0, 0.0), (200.0, 0.0)),
                ("right", (false, false), (0.0, 0.0), (0.0, 0.0)),
                ("top", (false, true), (0.0, -100.0), (0.0, 100.0)),
                ("bottom", (false, false), (0.0, 0.0), (0.0, 0.0)),
                (
                    "top-left corner",
                    (true, true),
                    (-200.0, -100.0),
                    (200.0, 100.0),
                ),
            ] {
                let old = fit(client, AT_REST, before, false, Fill::Anchor, pins);
                assert_eq!(
                    old.offset,
                    Point::from(larger),
                    "shrunk from the {side}, the old picture keeps its far side"
                );
                assert_eq!(old.crop, Some(client), "and is cut on the {side}");
                let new = fit(client, AT_REST, after, false, Fill::Anchor, pins);
                assert_eq!(
                    new.offset,
                    Point::from(smaller),
                    "shrunk from the {side}, the answer is against the far side"
                );
                assert_eq!(new.crop, None, "and inside the rectangle");
                for unanchored in [Fill::Crop, STRETCH] {
                    assert_eq!(
                        fit(client, AT_REST, after, false, unanchored, pins).offset,
                        Point::default(),
                        "{unanchored:?} keeps the corner, shrunk from the {side}"
                    );
                }
            }
            // On screen, the left shrink: the right 600 of the old 800, at the
            // rectangle, and not the left 600.
            let old = fit(client, AT_REST, before, false, Fill::Anchor, (true, false));
            assert_eq!(
                drawn(Surface::new((-100, 50), (800, 500)), (-100, 50), old, 1.0),
                Some(cut(
                    physical(100, 50, 600, 400),
                    part(200.0, 0.0, 600.0, 400.0)
                )),
                "the part of the picture against the edge standing still"
            );
        }

        /// **#233: a picture larger than its rectangle is cut to it, never
        /// squashed into it, tiled or not.** A floating window's drag and a
        /// held tile (which `Solium::tile_of` answers no tile for) used to
        /// scale such a picture down to fit, `stretch` still does; the
        /// others cut it at its own size, at either output scale.
        #[test]
        fn a_picture_larger_than_its_rectangle_is_cut_and_never_squashed() {
            let client = logical(100.0, 50.0, 300.0, 200.0);
            let committed = Size::from((450, 600));
            assert_eq!(
                fit(client, AT_REST, committed, false, STRETCH, STILL).factor,
                Scale::from((300.0 / 450.0, 200.0 / 600.0)),
                "a stretch squashes what nothing tiles"
            );
            for fill in UNSCALED {
                let fitted = fit(client, AT_REST, committed, false, fill, STILL);
                assert_eq!(fitted.factor, Scale::from((1.0, 1.0)), "{fill:?}");
                assert_eq!(fitted.crop, Some(client), "{fill:?}");
                assert_eq!(fitted.shown, Size::from((300, 200)), "{fill:?}");
                assert_eq!(
                    drawn(Surface::new((100, 50), (450, 600)), (100, 50), fitted, 1.0),
                    Some(cut(physical(100, 50, 300, 200), buffer(300.0, 200.0))),
                    "{fill:?}: 300x200 of screen, the first 300x200 of buffer"
                );
                assert_eq!(
                    drawn(
                        Surface::new((200, 100), (900, 1200)),
                        (200, 100),
                        fitted,
                        2.0
                    ),
                    Some(cut(physical(200, 100, 600, 400), buffer(600.0, 400.0))),
                    "{fill:?}: and at 2x the same cut in twice the pixels"
                );
            }
        }

        /// **#233: a grow is not stretched either way.** Before the client
        /// answers, its old picture is smaller than the rectangle the glide
        /// has reached and a stretch scaled it up; anchored it stays at its
        /// own size against the edge standing still. If the client answers
        /// first, its new picture is larger than the rectangle still growing
        /// and is cut, not squashed.
        #[test]
        fn a_grow_is_not_stretched_either_way() {
            let client = logical(0.0, 0.0, 700.0, 400.0);
            let old = Size::from((500, 400));
            assert_eq!(
                fit(client, AT_REST, old, true, STRETCH, STILL).factor,
                Scale::from((700.0 / 500.0, 1.0)),
                "a stretch scaled the old picture up into the growing glide"
            );
            let grown = fit(client, AT_REST, old, true, Fill::Anchor, STILL);
            assert_eq!(grown.factor, Scale::from((1.0, 1.0)));
            assert_eq!(grown.crop, None);
            assert_eq!(
                uncovered(
                    client,
                    client.loc + grown.offset,
                    (500.0, 400.0).into(),
                    1.0
                ),
                [
                    physical(0, 0, 700, 0),
                    physical(0, 400, 700, 0),
                    physical(0, 0, 0, 400),
                    physical(500, 0, 200, 400),
                ],
                "the 200 it has grown by is the background's, on the right"
            );
            assert_eq!(
                fit(client, AT_REST, old, true, Fill::Anchor, (true, false)).offset,
                Point::from((200.0, 0.0)),
                "grown from the left, the old picture keeps to the right"
            );
            let answered = fit(
                client,
                AT_REST,
                Size::from((900, 400)),
                false,
                Fill::Anchor,
                STILL,
            );
            assert_eq!(answered.factor, Scale::from((1.0, 1.0)));
            assert_eq!(answered.crop, Some(client), "the answer, cut to the glide");
        }

        /// **#125: a one-pixel buffer is not stretched across its pane.**
        ///
        /// A live trace caught clients committing `524x1` mid-drag, which the
        /// bridge drew at the pane's full height, 342x. Under the default fill
        /// it is a one-pixel strip at the top of the pane -- at the bottom
        /// when the drag pulls the top edge -- and the background is the
        /// rest.
        #[test]
        fn a_one_pixel_buffer_is_not_stretched_across_its_pane() {
            let client = logical(0.0, 0.0, 524.0, 342.0);
            let sliver = Size::from((524, 1));
            assert_eq!(
                fit(client, AT_REST, sliver, false, STRETCH, STILL).factor,
                Scale::from((1.0, 342.0)),
                "a stretch draws the row of pixels 342 times over"
            );
            let held = fit(client, AT_REST, sliver, false, Fill::default(), STILL);
            assert_eq!(held.factor, Scale::from((1.0, 1.0)));
            assert_eq!(held.crop, None);
            assert_eq!(
                uncovered(client, client.loc + held.offset, (524.0, 1.0).into(), 1.0)[1],
                physical(0, 1, 524, 341),
                "the background below the one row"
            );
            assert_eq!(
                fit(
                    client,
                    AT_REST,
                    sliver,
                    false,
                    Fill::default(),
                    (false, true)
                )
                .offset,
                Point::from((0.0, 341.0)),
                "a pulled top edge keeps it against the bottom"
            );
        }

        /// **The background fills what the picture leaves uncovered, and
        /// nothing else**: the bands above and below the whole width, the
        /// strips beside it between them, none where the picture reaches,
        /// all of the rectangle when the picture misses it. Rounded to the
        /// output's pixels with the picture rounded inward, so a strip ends
        /// under the picture's edge and never a pixel short of it.
        #[test]
        fn the_background_fills_what_the_picture_leaves_uncovered() {
            let client = logical(100.0, 50.0, 600.0, 400.0);
            assert_eq!(
                uncovered(client, (300.0, 50.0).into(), (400.0, 300.0).into(), 1.0),
                [
                    physical(100, 50, 600, 0),
                    physical(100, 350, 600, 100),
                    physical(100, 50, 200, 300),
                    physical(700, 50, 0, 300),
                ],
                "anchored right and top: the left strip and the band below"
            );
            for scale in [1.0, 1.25, 1.5, 2.0] {
                assert!(
                    uncovered(client, client.loc, client.size, scale)
                        .iter()
                        .all(|strip| strip.size.w == 0 || strip.size.h == 0),
                    "a picture that covers its rectangle leaves nothing, at {scale}x"
                );
            }
            assert_eq!(
                uncovered(client, client.loc, (599.7, 400.0).into(), 1.25)[3]
                    .size
                    .w,
                0,
                "nor one a rounding short of it"
            );
            assert_eq!(
                uncovered(client, (900.0, 50.0).into(), (100.0, 100.0).into(), 1.0)[0],
                physical(100, 50, 600, 400),
                "a picture wholly off its rectangle leaves all of it"
            );
            assert_eq!(
                uncovered(client, (300.25, 50.0).into(), (399.5, 400.0).into(), 2.0)[2],
                physical(200, 100, 401, 800),
                "at 2x, a fractional picture: the strip reaches its first whole pixel"
            );
            assert_eq!(
                uncovered(client, (100.0, 50.2).into(), (600.0, 0.5).into(), 1.0),
                [
                    physical(100, 50, 600, 0),
                    physical(100, 50, 600, 400),
                    physical(100, 50, 0, 0),
                    physical(700, 50, 0, 0),
                ],
                "a picture less than a pixel tall: the band below is all of it, and \
                 no strip has a negative size"
            );
        }

        /// **`resize.background` is the colour the strips are drawn in**,
        /// premultiplied, at the frame's opacity; `none` draws none.
        #[test]
        fn the_background_is_drawn_in_its_configured_colour() {
            assert_eq!(background_colour(None, 1.0), None);
            assert_eq!(
                background_colour(Some([255, 0, 51, 255]), 1.0),
                Some([1.0, 0.0, 0.2, 1.0])
            );
            let Some([red, green, _, alpha]) = background_colour(Some([255, 255, 0, 255]), 0.5)
            else {
                panic!("a colour is drawn");
            };
            assert!(
                (alpha - 0.5).abs() < f32::EPSILON,
                "half faded, half the alpha"
            );
            assert!((red - 0.5).abs() < f32::EPSILON && (green - 0.5).abs() < f32::EPSILON);
        }

        /// A 400x300 picture anchored to the right and the top of a 600x400
        /// client at (100, 50), as an anchored shrink from the left leaves
        /// it, and the background's strips round it: a strip on the left and
        /// a band below.
        fn anchored_right_and_top() -> (crate::render::Placed, Size<i32, Logical>) {
            let client = logical(100.0, 50.0, 600.0, 400.0);
            let committed = Size::from((400, 300));
            let fitting = fit(
                client,
                AT_REST,
                committed,
                false,
                Fill::Anchor,
                (true, false),
            );
            (
                crate::render::Placed {
                    client,
                    origin: client.loc + fitting.offset,
                    fit: fitting,
                },
                committed,
            )
        }

        /// **#233: a rounded client's background is cut at its corners.**
        ///
        /// The strips are drawn through the client's own rounded rectangle,
        /// in the output's pixels where they are drawn: they claim none of a
        /// corner's square, and the program reads each strip's pixels in the
        /// client's coordinates, so it is the client's corners that are cut
        /// and not the strip's own, which would cut it mid-window. Drawn
        /// square, as they were, they stood past a rounded window's corners
        /// in the background's colour on every frame of a glide or a drag,
        /// over what its style draws behind them. At 2x the same, in twice
        /// the pixels; a client its style does not round has no clip.
        #[test]
        fn a_rounded_clients_background_is_cut_at_its_corners() {
            use solium_effects::fragment::{Corners, Effect};
            let (placed, committed) = anchored_right_and_top();
            let rounded = Effect::rounded(Corners::all(12.0));
            let strips = |scale, rounding| {
                crate::render::background_strips(
                    Some([0x1c, 0x1c, 0x1c, 0xff]),
                    &placed,
                    committed,
                    1.0,
                    scale,
                    rounding,
                )
                .expect("a colour draws strips")
            };
            let drawn = strips(1.0, Some(rounded));
            let clip = drawn
                .clip
                .expect("a rounded client's strips are drawn through its clip");
            assert_eq!(
                clip.rect,
                Rectangle::new((100.0, 50.0).into(), (600.0, 400.0).into()),
                "the client's rectangle, in the output's pixels"
            );
            assert_eq!(clip.radii, crate::pass::physical_radii(rounded, 1.0));
            assert_eq!(
                clip.factor,
                Scale::from((1.0, 1.0)),
                "a strip is not rescaled"
            );
            let [top, bottom, left, right] = drawn.rects;
            assert!(top.is_empty() && right.is_empty(), "{:?}", drawn.rects);
            assert_eq!(left, physical(100, 50, 200, 300));
            assert_eq!(bottom, physical(100, 350, 600, 100));

            // The left strip holds the client's top-left corner and the band
            // below both bottom corners; each claims all but those squares,
            // in its own coordinates.
            let claimed = |strip: Rectangle<i32, Physical>| {
                crate::clip::claim(&[Rectangle::from_size(strip.size)], strip.loc, 1.0, &clip)
            };
            let corner = |claim: &[Rectangle<i32, Physical>], square: Rectangle<i32, Physical>| {
                claim.iter().any(|claimed| claimed.overlaps(square))
            };
            let left_claim = claimed(left);
            assert!(
                !corner(&left_claim, physical(0, 0, 12, 12)),
                "the client's top-left corner is not claimed: {left_claim:?}"
            );
            assert!(
                corner(&left_claim, physical(0, 12, 12, 12)),
                "{left_claim:?}"
            );
            let bottom_claim = claimed(bottom);
            assert!(
                !corner(&bottom_claim, physical(0, 88, 12, 12))
                    && !corner(&bottom_claim, physical(588, 88, 12, 12)),
                "nor are its bottom corners: {bottom_claim:?}"
            );

            // And the program measures the corners from the client's.
            let to_client = |strip: Rectangle<i32, Physical>, (u, v): (f32, f32)| {
                let matrix = crate::clip::solid_to_geo(strip, &clip).expect("an invertible strip");
                (
                    matrix[0] * u + matrix[3] * v + matrix[6],
                    matrix[1] * u + matrix[4] * v + matrix[7],
                )
            };
            let near = |(x, y): (f32, f32), (a, b): (f32, f32)| {
                (x - a).abs() < 1e-3 && (y - b).abs() < 1e-3
            };
            assert!(
                near(to_client(left, (0.0, 0.0)), (0.0, 0.0)),
                "the left strip starts at the client's top-left corner"
            );
            assert!(
                near(to_client(bottom, (0.0, 0.0)), (0.0, 300.0))
                    && near(to_client(bottom, (1.0, 1.0)), (600.0, 400.0)),
                "and the band below ends at its bottom-right"
            );

            let twice = strips(2.0, Some(rounded)).clip.expect("clipped at 2x too");
            assert_eq!(
                twice.rect,
                Rectangle::new((200.0, 100.0).into(), (1200.0, 800.0).into())
            );
            assert_eq!(twice.radii, crate::pass::physical_radii(rounded, 2.0));
            assert_eq!(
                strips(1.0, None).clip,
                None,
                "a square client's strips are square"
            );
        }

        /// **The background is drawn where the picture does not reach, and
        /// only there** (#233): one solid per strip that is not empty, in the
        /// background's colour, each under the pane's own identity for that
        /// side so a strip that moves is damaged where it was; and nothing at
        /// all for `"none"`. `panes` puts them behind the client, which needs
        /// a renderer and is read.
        #[test]
        fn the_background_is_drawn_where_the_picture_does_not_reach() {
            let (placed, committed) = anchored_right_and_top();
            let pane = crate::pane::Pane::loading(
                "kitty",
                None,
                Rectangle::new((100, 50).into(), (600, 400).into()),
                std::path::PathBuf::new(),
                None,
                std::time::Duration::ZERO,
            );
            let colour = Some([0x10, 0x20, 0x30, 0xff]);
            let elements = crate::render::background_elements(
                &pane,
                crate::render::background_strips(colour, &placed, committed, 1.0, 1.0, None),
                None,
            );
            let drawn: Vec<(Rectangle<i32, Physical>, [f32; 4], Id)> = elements
                .iter()
                .map(|element| match element {
                    crate::render::Element::Solid(solid) => {
                        let painted = solid.color();
                        (
                            solid.geometry(Scale::from(1.0)),
                            [painted.r(), painted.g(), painted.b(), painted.a()],
                            solid.id().clone(),
                        )
                    }
                    _ => panic!("a square client's strip is a plain solid"),
                })
                .collect();
            let [_, below, beside, _] = pane.background();
            let wanted = background_colour(colour, 1.0).expect("a colour");
            assert_eq!(
                drawn,
                vec![
                    (physical(100, 350, 600, 100), wanted, below.clone()),
                    (physical(100, 50, 200, 300), wanted, beside.clone()),
                ],
                "the band below and the strip on the left, and no empty strip"
            );
            assert!(
                crate::render::background_elements(
                    &pane,
                    crate::render::background_strips(None, &placed, committed, 1.0, 1.0, None),
                    None,
                )
                .is_empty(),
                "\"none\" draws nothing"
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
                STRETCH,
                STILL,
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

    /// **The pointer is drawn where its flight has got to, at the instant
    /// the screen is drawn at** (#220): its picture is placed at
    /// `Solium::drawn_pointer_at` the `now` [`super::elements`] sampled for
    /// everything else on the screen, which is where a warp's flight has
    /// carried it then, and not at the pointer's own location, which a warp
    /// moves at once.
    ///
    /// A source check, as the drag icon's test below is, and for its reason:
    /// `cursor` needs a renderer this binary cannot make. It proves only that
    /// `cursor` reads the drawn place at the frame's instant;
    /// `drawn_pointer_at` itself is `state::tests::real_client::follow`'s.
    #[test]
    fn the_pointer_is_drawn_where_its_flight_has_got_to() {
        let source = include_str!("render.rs");
        let start = source
            .find("\nfn cursor(")
            .expect("render.rs still draws the pointer in `cursor`");
        let body = &source[start..];
        let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
        assert!(
            body.contains("state.drawn_pointer_at(now)") && !body.contains("current_location()"),
            "`cursor` does not place the pointer where its flight has got to at the frame's instant"
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

    /// **The lock's exit is drawn over the session and under the pointer**:
    /// `elements` emits it after the pointer, which is above everything, and
    /// before the lock's own branch and everything of the session's, so the
    /// desktop is under it and nothing of the session's is over it. Like the
    /// test above, this pins only the order of the statements.
    #[test]
    fn the_lock_s_exit_is_drawn_over_the_session_and_under_the_pointer() {
        let source = include_str!("render.rs");
        let at = |needle: &str| {
            source
                .find(needle)
                .unwrap_or_else(|| panic!("`elements` no longer has {needle:?}"))
        };
        let pointer = at("elements.extend(cursor(");
        let exit = at("&& state.lock_scene_over_desktop(&output, now)");
        let lock = at("if let Some(lock) = state.lock.as_ref() {");
        let icon = at("elements.extend(drag_icon(");
        assert!(
            pointer < exit && exit < lock && lock < icon,
            "the exit is drawn at byte {exit}, not between the pointer ({pointer}) \
             and the session ({lock}, {icon})"
        );
    }

    /// **Through the lock's entry the session is drawn under the scene, and
    /// never in a capture**: the lock's branch asks
    /// `Solium::desktop_under_lock` with the picture's `capture`, and returns
    /// with the backdrop unless it says yes. What it says is
    /// `state::tests::real_client::lock_focus::native::entry`'s; like the
    /// tests above, this pins only the source.
    #[test]
    fn the_session_is_under_the_lock_only_through_its_entry_and_never_in_a_capture() {
        let source = include_str!("render.rs");
        let at = |needle: &str| {
            source
                .find(needle)
                .unwrap_or_else(|| panic!("`elements` no longer has {needle:?}"))
        };
        let lock = at("if let Some(lock) = state.lock.as_ref() {");
        let asked = at("state.desktop_under_lock(output, now, capture)");
        let guard = at("if !desktop {");
        let returned = lock
            + source[lock..]
                .find("return elements;")
                .expect("the lock guard still returns");
        assert!(
            lock < asked && asked < guard && guard < returned,
            "the backdrop's return is not guarded by the entry's answer for this picture"
        );
        assert!(
            include_str!("screencopy.rs").contains("capture: true,"),
            "a screencopy is not drawn as a capture"
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

    /// **A client faded out draws its pane's layers and no client** (#296):
    /// the flat path's walk, each piece at what `drawn_alpha` gives it. At a
    /// `client_opacity` of 0 the layers are drawn at the pane's opacity and
    /// nothing of the client is, its slots included; the pane's own slot
    /// keeps the pane's. Half way, the client and its slot are at the pane's
    /// opacity times its own.
    #[test]
    fn a_client_faded_out_draws_its_panes_layers_and_no_client() {
        use crate::effect::plan::PaneSlot;
        use crate::effect::rules::Slot;
        on_the_qt_thread(|| {
            let decoration = example();
            let has = |part: PaneSlot, slot: Slot| slot == Slot::Front && part != PaneSlot::Popups;
            let drawn = |client_opacity: f32| {
                let frame = crate::present::Frame::real(smithay::utils::Rectangle::new(
                    (0, 0).into(),
                    (300, 200).into(),
                ))
                .with_opacity(0.8)
                .with_client_opacity(client_opacity);
                let mut order: Vec<(String, f32)> = Vec::new();
                super::pane_walk(&mut order, &has, |into, piece| {
                    let Some(alpha) = super::drawn_alpha(&frame, piece) else {
                        return;
                    };
                    match piece {
                        super::Piece::Layers(depth) => into.extend(
                            decoration
                                .layers_at(depth)
                                .map(|name| (name.to_owned(), alpha)),
                        ),
                        super::Piece::Client => into.push(("<client>".to_owned(), alpha)),
                        super::Piece::Slot(part, slot) => {
                            into.push((format!("<{part:?} {slot:?}>"), alpha));
                        }
                    }
                });
                order
            };
            let at = |name: &str, alpha: f32| (name.to_owned(), alpha);
            assert_eq!(
                drawn(0.0),
                [
                    at("<Pane Front>", 0.8),
                    at("spikes", 0.8),
                    at("bar", 0.8),
                    at("glow", 0.8)
                ]
            );
            let half = 0.8_f32 * 0.5;
            assert_eq!(
                drawn(0.5),
                [
                    at("<Pane Front>", 0.8),
                    at("spikes", 0.8),
                    at("bar", 0.8),
                    at("<Client Front>", half),
                    at("<client>", half),
                    at("glow", 0.8)
                ]
            );
        });
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

    /// **Client slots bracket the client, and a `replace` takes its
    /// place**, its `front` and `behind` still around it (#389).
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
            // And a replaced client is still bracketed by its other slots
            // (#389), as every part is (`bracket`).
            assert_eq!(
                walked(&decoration, &|part, _| part == PaneSlot::Client),
                [
                    "spikes",
                    "bar",
                    "<Client Front>",
                    "<Client Replace>",
                    "<Client Behind>",
                    "glow"
                ]
            );
        });
    }

    /// **Pane slots are around the sandwich**; the popups are not in the
    /// walk at all (they go in ahead of it), so a pane's `front` is below them.
    /// A pane `replace` takes the sandwich's place, inside its `front` and
    /// `behind`.
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
            assert_eq!(
                walked(&decoration, &|part, _| part == PaneSlot::Pane),
                ["<Pane Front>", "<Pane Replace>", "<Pane Behind>"]
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
            let ready = super::slot_ready(&slots, pane, None);
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
            let (drawing, origin) =
                super::pane_drawing_at(outer, insets, at.into(), scale, std::time::Duration::ZERO);
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
            let drawing = super::layer_drawing_at(
                canvas,
                outer,
                (pad, pad).into(),
                scale,
                std::time::Duration::ZERO,
            );
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
        fn scenes(&mut self, _slots: &crate::effect::plan::Slots) {
            self.did.push("scenes".to_owned());
        }
        fn xray(&mut self, _slots: &crate::effect::plan::Slots) {
            self.did.push("xray".to_owned());
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
                "scenes",
                "xray",
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

    /// **The pass is in order** (Ruling F31): scenes and xray after resolve
    /// and before any capture or chain; the warps after the chains.
    #[test]
    fn prepare_renders_scenes_then_xray_before_the_chains() {
        let mut recorder = Recorder {
            pane: crate::pane::PaneId::from_raw(1),
            did: Vec::new(),
            walked: Vec::new(),
        };
        let _ = super::effect_phases(&mut recorder);
        let at = |phase: &str| recorder.did.iter().position(|did| did == phase);
        assert!(at("resolve") < at("scenes") && at("scenes") < at("xray"));
        assert!(at("xray") < at("captures Inner") && at("chains Whole") < at("warps"));
    }

    fn mkey(upto: u64) -> crate::effect::memo::MemoKey {
        crate::effect::memo::MemoKey {
            monitor: "DP-1".to_owned(),
            cut: crate::effect::xray::Cut::Background,
            upto,
        }
    }

    /// **A memo runs when its xray was redrawn, and not otherwise**; two
    /// slots wanting one memo run it once.
    #[test]
    fn a_memo_runs_when_its_xray_was_redrawn_and_once_for_every_slot() {
        let (mut memos, mut xrays) = (
            crate::effect::memo::Memos::default(),
            crate::effect::xray::Xrays::default(),
        );
        xrays.want("DP-1", crate::effect::xray::Cut::Background);
        memos.want(&mkey(7), 1);
        let wanted = [mkey(7), mkey(7)];
        assert!(
            super::memos_to_run(&memos, &xrays, &wanted).is_empty(),
            "a memo ran over a cut not drawn yet"
        );
        xrays.drawn("DP-1", crate::effect::xray::Cut::Background, true);
        memos.want(&mkey(7), 1);
        assert_eq!(super::memos_to_run(&memos, &xrays, &wanted), vec![mkey(7)]);
        memos.ran(
            &mkey(7),
            xrays
                .generation("DP-1", crate::effect::xray::Cut::Background)
                .unwrap_or(0),
        );
        assert!(
            super::memos_to_run(&memos, &xrays, &wanted).is_empty(),
            "re-ran over the same xray"
        );
        xrays.drawn("DP-1", crate::effect::xray::Cut::Background, false);
        assert!(
            super::memos_to_run(&memos, &xrays, &wanted).is_empty(),
            "a kept xray re-ran it"
        );
        xrays.drawn("DP-1", crate::effect::xray::Cut::Background, true);
        assert_eq!(super::memos_to_run(&memos, &xrays, &wanted), vec![mkey(7)]);
    }

    /// **An empty suffix runs no pass: its result is a crop of the memo**;
    /// a suffix that reads the part runs; a place whose memo was not held
    /// (over `effects.memo.budget_mib`) runs the whole plan (amended
    /// 2026-10-08, configurable).
    #[test]
    fn an_empty_suffix_is_a_crop_and_a_suffix_runs() {
        use crate::effect::memo::tests::chain;
        let plain = solium_effects::stage::Memo {
            prefix: None,
            suffix: chain(&[]),
            exports: vec![solium_effects::stage::Export {
                step: 5,
                name: "memo:5".to_owned(),
                scale: 1.0,
            }],
        };
        assert_eq!(
            super::t2_work(&plain, true),
            super::T2Work::Crop(plain.exports[0].clone())
        );
        let glass = solium_effects::stage::Memo {
            prefix: None,
            suffix: chain(&["tint"]),
            exports: Vec::new(),
        };
        assert_eq!(super::t2_work(&glass, true), super::T2Work::Suffix);
        assert_eq!(super::t2_work(&plain, false), super::T2Work::Whole);
        assert_eq!(super::t2_work(&glass, false), super::T2Work::Whole);
    }

    /// **A crop is the part's box in the memo's own pixels.**
    #[test]
    fn a_crop_is_the_parts_box_in_the_memos_pixels() {
        let map = crate::effect::run::BoxMap {
            offset: [0.25, 0.5],
            scale: [0.125, 0.25],
        };
        assert_eq!(
            super::crop_of(map, (2560, 1440)),
            smithay::utils::Rectangle::new((640.0, 720.0).into(), (320.0, 360.0).into())
        );
    }

    /// A T2 place on `DP-1`, its box in the monitor's pixels at `x`.
    fn t2_place(x: i32) -> crate::effect::plan::T2Place {
        crate::effect::plan::T2Place {
            monitor: "DP-1".to_owned(),
            cut: crate::effect::xray::Cut::Background,
            padded: smithay::utils::Rectangle::new((50, 25).into(), (100, 50).into()),
            screen: smithay::utils::Rectangle::new((x, 50).into(), (200, 100).into()),
            map: crate::effect::run::BoxMap {
                offset: [0.05, 0.05],
                scale: [0.1, 0.1],
            },
            own: (200, 100).into(),
            part: crate::effect::plan::PartBox::around((100, 60).into(), 0, [0.0; 4]),
        }
    }

    /// **A T2 key moves with a drag finer than an xray pixel** (Ruling F37):
    /// it holds the padded box in the monitor's own pixels, not in the
    /// xray's, so at `effects.xray.scale` 0.5 (or the half rung) a window
    /// moved by one monitor pixel re-runs its suffix even where its box in
    /// the xray did not move.
    #[test]
    fn a_t2_key_moves_with_a_drag_finer_than_an_xray_pixel() {
        use crate::effect::memo::tests::chain;
        let (memos, xrays) = (
            crate::effect::memo::Memos::default(),
            crate::effect::xray::Xrays::default(),
        );
        let blur = crate::effect::plan::BoundChain::t2_for_test(chain(&["down", "up"]), 7);
        let work = super::T2Work::Suffix;
        let key = |place: &crate::effect::plan::T2Place| {
            super::t2_key(
                &memos, &xrays, place, &blur, &work, [0.0; 4], [0.0; 4], None,
            )
        };
        let (here, there) = (t2_place(100), t2_place(101));
        assert_eq!(here.padded, there.padded, "the premise: one xray pixel");
        assert_ne!(key(&here), key(&there), "a one-pixel drag kept the key");
    }

    /// **A T2 key reads each memo by its prefix and its generation**: two
    /// chains whose memos have the same generation (each ran once) but are
    /// other memos key apart, so a place that reads another memo re-runs.
    #[test]
    fn a_t2_key_reads_each_memo_by_its_prefix_and_generation() {
        use crate::effect::memo::tests::chain;
        let (mut memos, mut xrays) = (
            crate::effect::memo::Memos::default(),
            crate::effect::xray::Xrays::default(),
        );
        xrays.drawn("DP-1", crate::effect::xray::Cut::Background, true);
        let soft = crate::effect::plan::BoundChain::t2_for_test(chain(&["down", "up"]), 7);
        let softer = crate::effect::plan::BoundChain::t2_for_test(chain(&["down2", "up2"]), 7);
        for chain in [&soft, &softer] {
            assert_eq!(chain.memo_keys().len(), 1, "the premise: one memo each");
            memos.want(&mkey(chain.memo_keys()[0]), 1);
            memos.ran(&mkey(chain.memo_keys()[0]), 1);
        }
        assert_ne!(
            soft.memo_keys(),
            softer.memo_keys(),
            "the premise: two memos"
        );
        let place = t2_place(100);
        let work = super::t2_work(soft.memo().expect("split"), true);
        let key = |chain: &crate::effect::plan::BoundChain| {
            super::t2_key(
                &memos, &xrays, &place, chain, &work, [0.0; 4], [0.0; 4], None,
            )
        };
        assert_ne!(
            key(&soft).memo,
            key(&softer).memo,
            "two memos of one generation read alike"
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

    /// **A glided part's box is where it is drawn**: its place in the pane
    /// carried by the frame's rectangle, not the layout's (Ruling F37,
    /// amended 2026-10-14, live under motion and warps).
    #[test]
    fn a_glided_parts_box_is_where_it_is_drawn() {
        use smithay::utils::{Logical, Rectangle};
        let outer = Rectangle::<i32, Logical>::new((1000, 100).into(), (400, 300).into());
        let titlebar = Rectangle::<i32, Logical>::new((1000, 100).into(), (400, 28).into());
        // Half way through a glide from (200, 100) to the layout's (1000, 100).
        let gliding = Rectangle::<f64, Logical>::new((600.0, 100.0).into(), (400.0, 300.0).into());
        assert_eq!(
            super::within(titlebar, outer, gliding),
            Rectangle::new((600.0, 100.0).into(), (400.0, 28.0).into())
        );
        // An open at half size about the pane's centre: the band halved with it.
        let opening = Rectangle::<f64, Logical>::new((1100.0, 175.0).into(), (200.0, 150.0).into());
        assert_eq!(
            super::within(titlebar, outer, opening),
            Rectangle::new((1100.0, 175.0).into(), (200.0, 14.0).into())
        );
        // At rest, the part itself.
        assert_eq!(
            super::within(titlebar, outer, outer.to_f64()),
            titlebar.to_f64()
        );
    }

    /// **A tilted part's beneath bounds are its own band**: under a matrix
    /// alone (one cell, the pane's corners) a titlebar's bounds are its
    /// band turned with the pane, not the whole pane's box, which a tint
    /// would cover and a `glass-rect` bezel bend through (amended
    /// 2026-10-14, live under motion and warps). Turned 0.17 rad about its
    /// centre, a 400×300 pane is 400·sin + 300·cos ≈ 363.3 tall, its 28 px
    /// titlebar 400·sin + 28·cos ≈ 95.3, both from the same top corner.
    #[test]
    fn a_tilted_parts_beneath_bounds_are_its_own_band() {
        let mut frame = crate::present::Frame::real(smithay::utils::Rectangle::new(
            (0, 0).into(),
            (400, 300).into(),
        ));
        frame.matrix = crate::mat4::Mat4::rotate_z(0.17);
        let titlebar = crate::warp::UnitRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 28.0 / 300.0,
        };
        let band = super::beneath_bounds(&frame, None, titlebar).expect("bounds");
        let whole =
            super::beneath_bounds(&frame, None, crate::warp::UnitRect::WHOLE).expect("bounds");
        assert!(
            (whole.size.h - 363.3).abs() < 1.0,
            "the pane's bounds are {} tall",
            whole.size.h
        );
        assert!(
            (band.size.h - 95.3).abs() < 1.0,
            "the titlebar's bounds are {} tall, not its band",
            band.size.h
        );
        assert!(
            (band.loc.y - whole.loc.y).abs() < 1.0,
            "the band does not start at the pane's top corner"
        );
    }

    /// **Inside a capture a beneath slot is a hole; on screen it never is**
    /// (amended 2026-10-14): a hole on screen would erase the frame's own
    /// pixels and draw black where the backdrop is. Both captures of a pane
    /// are inside (the warp's and a whole pane's self or mask capture), and
    /// with a place lacking its texture the slot is skipped, not holed.
    #[test]
    fn a_beneath_slot_is_a_hole_only_inside_a_capture() {
        use super::{SlotDraw, Within, slot_draw};
        assert_eq!(slot_draw(Within::Capture, true, true), SlotDraw::Hole);
        assert_eq!(slot_draw(Within::Capture, false, true), SlotDraw::Result);
        assert_eq!(slot_draw(Within::Screen, true, true), SlotDraw::Result);
        assert_eq!(slot_draw(Within::Screen, false, true), SlotDraw::Result);
        // A monitor with no texture beneath: no hole, which would show the
        // screen below there (amended 2026-10-14).
        assert_eq!(slot_draw(Within::Capture, true, false), SlotDraw::Skip);
    }

    /// **The walk draws at the pass's instant**, the one its T2 places were
    /// measured at, however late `elements` runs; outside a pass, at the
    /// clock (amended 2026-10-14, live under motion and warps).
    #[test]
    fn the_walk_draws_at_the_passs_instant() {
        let (pass, later) = (
            std::time::Duration::from_millis(1000),
            std::time::Duration::from_millis(1004),
        );
        assert_eq!(super::walk_instant(Some(pass), later), pass);
        assert_eq!(super::walk_instant(None, later), later);
    }
}
