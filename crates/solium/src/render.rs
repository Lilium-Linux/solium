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
        gles::{GlesRenderer, GlesTexture},
        utils::CommitCounter,
    },
    desktop::{LayerSurface, PopupManager, Window, layer_map_for_output},
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::compositor::with_states,
};

use crate::{
    layer,
    pane::Pane,
    present,
    stack::{Band, Owner},
    state::Solium,
    style::Depth,
};

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
/// Built in two phases: every capture's element list first, which can run Qt,
/// then every capture drawn on one bound carrier, which must not
/// (`offscreen::draw`).
#[derive(Default)]
pub(crate) struct Prepared {
    warps: Vec<(Window, GlesTexture, crate::warp::Program, Id, CommitCounter)>,
    overs: Vec<(
        Window,
        GlesTexture,
        crate::warp::Program,
        Id,
        CommitCounter,
        crate::warp::UnitRect,
    )>,
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

impl Prepared {
    /// Lend the texture captured for `window`, the program to draw it
    /// through, and the id and commit its warp carries, if there is one.
    ///
    /// Lent rather than taken: with more than one monitor `elements` runs once
    /// per output, and a texture removed by the first one would leave a
    /// deformed window undrawn on every other screen. `GlesTexture` is a
    /// handle, so the clone is a refcount, and the program is GL names.
    fn warp(
        &self,
        window: &Window,
    ) -> Option<(GlesTexture, crate::warp::Program, Id, CommitCounter)> {
        self.warps.iter().find(|(each, ..)| each == window).map(
            |(_, texture, program, id, commit)| (texture.clone(), *program, id.clone(), *commit),
        )
    }

    /// Lend the capture of `window`'s popups, for the warp drawn in front of
    /// its own, with the part of the pane's unit square it covers. Lent for
    /// `warp`'s reason. `None` when it has no popups open.
    /// `dev/present-check.sh`'s `menu` case draws it.
    fn over(
        &self,
        window: &Window,
    ) -> Option<(
        GlesTexture,
        crate::warp::Program,
        Id,
        CommitCounter,
        crate::warp::UnitRect,
    )> {
        self.overs.iter().find(|(each, ..)| each == window).map(
            |(_, texture, program, id, commit, part)| {
                (texture.clone(), *program, id.clone(), *commit, *part)
            },
        )
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
    // Effects compile here, between frames, where the context is current:
    // nothing at all when no effect is wanted (spec §8.4,
    // `effect::host::tests::an_empty_host_touches_no_gl`). The formats a
    // plan may draw into are probed first, once, and only once something is
    // wanted (`effect::host::tests::the_formats_are_probed_once_and_only_while_something_is_wanted`;
    // the probe itself is wirecheck case 12c's). A changed answer rebinds
    // the rules after the frame (`note_formats`).
    if !state.effects.is_idle() {
        if state.effects.wants_formats() {
            let found = crate::pool::probe_formats(renderer);
            note_formats(state, found);
        }
        state
            .effects
            .compile_pending(&mut crate::effect::GlCompiler(renderer));
    }
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

    let mut warps = Vec::new();
    let mut overs = Vec::new();
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
        if !state
            .pane_outer_of(pane)
            .is_some_and(|slot| state.on_any_output(slot))
        {
            release(state);
            continue;
        }
        // The anchor is resolved here, and not merely tested for presence: a
        // deform aimed at a pane that has closed draws flat, and capturing a
        // texture for it would be a megabyte a frame spent on a warp that
        // `elements` has already decided not to do.
        let frame = state.drawn(pane, outer);
        // At *its own monitor's* scale. One frame can span monitors at
        // different scales, and a texture taken at 1x and drawn on a 2x screen
        // is the blur this whole change exists to remove.
        //
        // A deformed window with no warp program takes the flat path below
        // instead of being captured for a warp that cannot be drawn:
        // `tests::a_window_whose_warp_has_no_program_is_drawn_flat`.
        let aimed = state.aimed_at_for(pane, frame.deform);
        let warped = !frame.matrix.is_identity() || aimed.is_some();
        let program = if warped {
            state.programs.warp(renderer)
        } else {
            None
        };
        let routed = route(warped, program.is_some());
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
        if let (Route::Warp, Some(program)) = (routed, program) {
            let scale = state.scale_of(outer);
            // What its mesh is drawn from, in global space: the warp's commit
            // moves when this does, as well as when its capture is redrawn.
            // `keyed::tests::a_warp_whose_mesh_moves_inside_the_same_bounds_is_given_a_new_commit`.
            let shape = Shape::of(&frame, aimed, scale);
            let job = crate::offscreen::pane_job(state, renderer, pane, &window, scale);
            // Its popups the same way, in a capture of their own that `panes`
            // draws in front of the pane's warp
            // (`tests::a_warped_panes_popups_are_in_front_of_it`), kept until
            // they commit:
            // `state::tests::real_client::a_commit_on_a_popup_makes_the_popups_capture_stale`.
            let over = if job.is_some() {
                crate::offscreen::over_job(state, renderer, pane, &window, scale)
            } else {
                None
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
                held.captures_mut().keep_only(kinds, pool);
            }
            if let Some(job) = job {
                // Already drawn from exactly this: no frame (`offscreen::kept`,
                // `state::tests::real_client::a_capture_whose_surface_tree_has_not_committed_is_not_drawn_again`).
                if let Some((texture, id, _commit)) = crate::offscreen::kept(state, &job) {
                    let commit = warp_commit(state, pane, crate::keyed::Kind::Pane, shape, false);
                    warps.push((window.clone(), texture, program, id, commit));
                } else {
                    jobs.push((job, Then::Warp(window.clone(), program, pane, shape)));
                }
            }
            if let Some((job, part)) = over {
                if let Some((texture, id, _commit)) = crate::offscreen::kept(state, &job) {
                    let commit = warp_commit(state, pane, crate::keyed::Kind::Over, shape, false);
                    overs.push((window, texture, program, id, commit, part));
                } else {
                    jobs.push((job, Then::Over(window, program, pane, shape, part)));
                }
            }
            continue;
        }
        release(state);
    }

    // Every list is built: draw them all, on one carrier.
    for (then, texture, id, _commit) in crate::offscreen::draw(state, renderer, jobs) {
        match then {
            Then::Warp(window, program, pane, shape) => {
                let commit = warp_commit(state, pane, crate::keyed::Kind::Pane, shape, true);
                warps.push((window, texture, program, id, commit));
            }
            Then::Over(window, program, pane, shape, part) => {
                let commit = warp_commit(state, pane, crate::keyed::Kind::Over, shape, true);
                overs.push((window, texture, program, id, commit, part));
            }
        }
    }
    Prepared { warps, overs }
}

/// What a capture becomes once drawn: `dev/fence-check.sh` has two warps,
/// `dev/present-check.sh`'s menu case a warp and its popups. A warp
/// carries its pane and the [`Shape`] its commit is moved by; its popups' warp
/// the part of the pane they cover as well.
#[derive(Debug)]
enum Then {
    Warp(Window, crate::warp::Program, crate::pane::PaneId, Shape),
    Over(
        Window,
        crate::warp::Program,
        crate::pane::PaneId,
        Shape,
        crate::warp::UnitRect,
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 18's slot plan reads a pane's style rules")
)]
pub(crate) fn style_rules(frame: &crate::pane::Frame) -> (&[crate::effect::rules::Rule], u32) {
    match frame {
        crate::pane::Frame::Styled(decoration) => {
            (decoration.rules(), decoration.rules_generation())
        }
        crate::pane::Frame::Pending | crate::pane::Frame::None => (&[], 0),
    }
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

/// A client's clip inside a warp's capture, which draws it at real size with
/// its tree at `origin`: its `geometry` (relative to the tree) there, cut to
/// the `hole` the frame leaves it from `origin`, which is what the capture
/// holds of it. `tests::in_a_capture_a_client_is_clipped_to_what_the_capture_holds_of_it`.
pub(crate) fn capture_clip(
    origin: Point<i32, Physical>,
    hole: Size<i32, Physical>,
    geometry: Rectangle<i32, Physical>,
    radii: solium_effects::fragment::Corners,
) -> crate::clip::Clip {
    let room = Rectangle::new(origin, hole);
    let client = Rectangle::new(origin + geometry.loc, geometry.size);
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
        Route::Flat => None,
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
    let mut animating = false;
    if let Some(decoration) = state.panes.get_mut(pane).and_then(Pane::decoration_mut) {
        // Already `Element`s: a layer is a memory buffer on the software path
        // and a texture on the GPU one, and which of the two it is is the
        // decoration's own business rather than this function's.
        //
        // And already an answer about whether they are still moving, out of the
        // same call: asking afterwards is asking the flag the draw just spent.
        // See [`Drawn`].
        animating = decoration.layer_elements(renderer, depth, &look, drawing, elements);
    }
    // Ask for another frame while the decoration is still moving. The client
    // has not damaged anything, so without this the next frame never comes and
    // the animation stops where it stood.
    if animating {
        state.redraw = true;
    }
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
        elements.extend(cursor(state, renderer, output_scale, scale, shift));
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
    // topmost first, and this turns each entry into elements and nothing more.
    let output = state.output_for(screen);
    for each in stacked(state, screen, now) {
        match each {
            Stacked::Layer(surface, geometry) => {
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
            Stacked::Surface(id, area, alpha) => {
                if let Some(output) = output.as_ref() {
                    elements.extend(scripted(
                        state, renderer, output, id, area, alpha, screen, now, scale,
                    ));
                }
            }
            Stacked::Panes(nodes) => {
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
        }
    }

    elements
}

/// One entry of a monitor's frame, before it is turned into elements.
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
/// frame and its deform's target, both in global space, moved onto the screen
/// together. `tests::a_genie_on_the_second_monitor_lands_on_its_target`,
/// `tests::a_genie_on_the_first_monitor_is_unchanged`.
pub(crate) fn warp_mesh_on(
    screen: Rectangle<i32, Logical>,
    frame: &present::Frame,
    aimed: Option<present::Aimed>,
    part: crate::warp::UnitRect,
    scale: f64,
) -> Option<crate::warp::Mesh> {
    let shift = Point::<f64, Logical>::from((-f64::from(screen.loc.x), -f64::from(screen.loc.y)));
    let onto = |rect: Rectangle<f64, Logical>| Rectangle::new(rect.loc + shift, rect.size);
    let aimed = aimed.map(|aimed| present::Aimed {
        to: onto(aimed.to),
        ..aimed
    });
    crate::warp::mesh_part(
        onto(frame.rect),
        part,
        frame.matrix,
        aimed,
        frame.pivot,
        scale,
    )
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
        // enlarged by a mode has its bleed enlarged with it.
        let reach = crate::decoration::spread(
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
        if frame.matrix.is_identity() && frame.deform.is_none() && !reach.overlaps(screen.to_f64())
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
                Piece::Layers(depth) => chrome(state, renderer, elements, pane, depth, drawing),
                Piece::Client => elements.extend(client.take().into_iter().flatten()),
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
                    chrome(state, renderer, elements, pane, depth, drawing);
                }
                Piece::Layers(_) => (),
                Piece::Client => scene(state, renderer, elements, pane, frame, now, scale),
            });
            continue;
        }

        let Some(window) = window else {
            continue;
        };

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
        let aimed = state.aimed_at_for(pane, frame.deform);
        if (!frame.matrix.is_identity() || aimed.is_some())
            && let Some((texture, program, id, commit)) = prepared.warp(&window)
            && let Some(pane_mesh) = warp_mesh_on(
                screen,
                &drawn_global,
                aimed,
                crate::warp::UnitRect::WHOLE,
                scale,
            )
        {
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
                        if let Some((over_texture, over_program, over_id, over_commit, part)) =
                            prepared.over(&window)
                            && let Some(mesh) =
                                warp_mesh_on(screen, &drawn_global, aimed, part, scale)
                        {
                            elements.push(Element::Warped(crate::warp::Warp::new(
                                over_id,
                                over_commit,
                                over_texture,
                                mesh,
                                frame.opacity,
                                over_program,
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
        let (popups, _) = popup_elements(renderer, &window, origin, output_scale, frame.opacity);
        // Scaled with the window and not cut to its tile: a menu has to reach
        // past its parent's tile, and a menu cut to it would lose every item
        // past the tile's edge. The window's own fit with the cut taken off,
        // which is `a_popup_reaches_past_its_parents_tile`. This is the only
        // place a toplevel's popups are drawn on this path -- the toplevel
        // itself is drawn from its own surface tree, which holds no popups;
        // see [`toplevel_elements`].
        elements.extend(popups.into_iter().filter_map(|element| {
            fitted(element, origin, fitting.uncut(), output_scale).map(Fitted::into_element)
        }));

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
        pane_pieces(elements, |elements, piece| match piece {
            Piece::Layers(depth) => chrome(state, renderer, elements, pane, depth, drawing),
            Piece::Client => {
                // Popups are not here: they went in above the whole sandwich,
                // before this walk started. See the comment there.

                // A surface's top-left is not the window's. A client that draws
                // its own decorations puts its drop shadow *outside* the window
                // geometry and tells us so through `set_window_geometry`;
                // drawing the surface at the window's position therefore lands
                // the shadow where the window should be and pushes the window
                // itself down and right by the shadow's width. That is what
                // made Firefox look both misplaced and shadowed. Popups already
                // did this; toplevels did not.
                let surface_origin =
                    origin - window.geometry().loc.to_physical_precise_round(scale);
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
        CursorImageStatus::Named(icon) => state
            .pointer
            .element(renderer, icon, location, scale)
            .into_iter()
            .collect(),
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
pub(crate) fn flat_window_elements(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    window: &Window,
    scale: f64,
) -> Vec<Element> {
    let mut elements = Vec::new();
    let Some(Flat { outer, insets }) = flat(state, window) else {
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
    // frame squared.
    let drawing = crate::decoration::Drawing {
        rect: present::logical((0.0, 0.0), (f64::from(outer.w), f64::from(outer.h))),
        outer,
        alpha: 1.0,
        scale,
    };

    // The client within it.
    let origin =
        Point::<i32, Logical>::from((insets.left, insets.top)).to_physical_precise_round(scale);

    // Asked before the walk, which holds the state.
    let rounded = pane.and_then(|pane| clipped(state, pane));

    // The same `PANE_ORDER` a flat window goes through, so a tilted window
    // carries its layers in the order it would have had standing still. A
    // second sequence of calls here is how a deformed window would come to have
    // its `above` layer underneath its client.
    pane_pieces(&mut elements, |elements, piece| match piece {
        Piece::Layers(depth) => {
            if let Some(pane) = pane {
                chrome(state, renderer, elements, pane, depth, drawing);
            }
        }
        // A style's rounding, drawn here as on the flat path, so a deformed
        // window keeps its corners: each surface through the clipped
        // programs, at real size, cut to what the capture holds of the client
        // (`tests::in_a_capture_a_client_is_clipped_to_what_the_capture_holds_of_it`).
        Piece::Client => {
            let surfaces = client_piece(renderer, window, origin, output_scale);
            match &rounded {
                Some((effect, programs)) => {
                    let clip = capture_clip(
                        origin,
                        Size::<i32, Logical>::from((
                            outer.w - insets.horizontal(),
                            outer.h - insets.vertical(),
                        ))
                        .to_physical_precise_round(scale),
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
    });
    elements
}

/// The rectangle a warped window is captured at, and where its client sits in
/// it. See [`flat`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Flat {
    /// The texture's size, in logical pixels: the pane's outer size.
    pub(crate) outer: Size<i32, Logical>,
    /// The frame's share of it, which puts the client's corner.
    pub(crate) insets: crate::decoration::Insets,
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
    match state.panes.of(window) {
        Some(pane) => Some(Flat {
            outer: state.pane_outer(pane).size,
            insets: state.insets_of(pane.id()),
        }),
        None => Some(Flat {
            outer: state.outer_geometry(window)?.size,
            insets: state.frame_insets(window),
        }),
    }
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
    let mut covered: Option<Rectangle<i32, Logical>> = None;
    let Some(surface) = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone())
    else {
        return (elements, covered);
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
        // The tree where it is drawn, not the popup's window geometry: a popup
        // that sets none has a zero-sized one, and what a capture of it must
        // hold is everything drawn above.
        // `state::tests::a_warped_panes_capture_holds_no_popups`.
        let rect = smithay::desktop::utils::bbox_from_surface_tree(
            popup.wl_surface(),
            offset - popup.geometry().loc,
        );
        covered = Some(covered.map_or(rect, |held| held.merge(rect)));
    }
    (elements, covered)
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
mod tests {
    use super::{Drawn, Fit, Fitted, Painted, by_depth, fit, fitted, origin_at, ratio};
    use crate::qml::qt_test::on_the_qt_thread;

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

    fn genie_to(
        to: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
    ) -> crate::present::Aimed {
        crate::present::Aimed {
            effect: solium_effects::Deform::Genie {
                progress: 1.0,
                spread: 1.4,
                axis: solium_effects::Axis::Down,
            },
            to,
        }
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
        let aimed = genie_to(crate::present::logical((2800.0, 1000.0), (120.0, 24.0)));
        let mesh = super::warp_mesh_on(
            screen,
            &frame,
            Some(aimed),
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
        let aimed = genie_to(crate::present::logical((880.0, 1000.0), (120.0, 24.0)));
        let on = super::warp_mesh_on(
            screen,
            &frame,
            Some(aimed),
            crate::warp::UnitRect::WHOLE,
            1.25,
        )
        .expect("a mesh");
        let direct = crate::warp::mesh(frame.rect, frame.matrix, Some(aimed), frame.pivot, 1.25)
            .expect("a mesh");
        let pairs = on.vertices().iter().zip(direct.vertices());
        assert!(
            pairs.clone().count() > 0 && pairs.into_iter().all(|(a, b)| a.x == b.x && a.y == b.y)
        );
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
                .find("let (popups, _) = popup_elements(")
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
}
