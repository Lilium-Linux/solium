//! Building the frame's render elements, through the presentation transform.
//!
//! This is the only place that knows a window may be drawn somewhere other
//! than where it lives. Every mode — overview, switcher, peek, genie — is
//! expressed by setting a target and reaches the screen through this function,
//! which is why they cannot animate inconsistently.
//!
//! Deliberately written against Smithay's `Renderer` traits and nothing lower.
//! Reaching into GLES specifics here is what would quietly close the option of
//! a Vulkan backend later — see `docs/spikes/2026-08-27-vulkan-on-smithay.md`.

use std::sync::Mutex;

use smithay::{
    backend::renderer::{
        element::{
            AsRenderElements, Id, Kind,
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::RescaleRenderElement,
        },
        gles::{GlesRenderer, GlesTexture},
        utils::CommitCounter,
    },
    desktop::{PopupManager, Window, layer_map_for_output},
    input::pointer::{CursorImageAttributes, CursorImageStatus},
    utils::Scale,
    wayland::compositor::with_states,
};

use crate::{layer, pane::Pane, present, state::Solium};

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
    Chrome = MemoryRenderBufferRenderElement<GlesRenderer>,
    Warped = crate::warp::Warp,
    /// A surface drawn straight, with no rescale wrapper: the offscreen pass
    /// draws at real size, so there is nothing to scale.
    Window2 = WaylandSurfaceRenderElement<GlesRenderer>,
    /// A flat colour. The only thing that draws one is the lock screen's
    /// backdrop, which has to be a real element rather than a clear colour
    /// because the lock client's surface is composited on top of it.
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

/// Textures captured for this frame, one per deformed window.
///
/// A deformed window is drawn flat into a texture of its own first. That pass
/// binds a framebuffer, so it cannot happen while the output's buffer is
/// already bound: it leaves GL pointing at the texture, and the whole frame --
/// including the deformed window -- lands there instead of on screen, which
/// looks exactly like a compositor that has frozen. So captures happen in
/// their own pass, before the backend binds anything, and `elements` only
/// spends what this collected.
#[derive(Default)]
pub(crate) struct Prepared {
    warps: Vec<(Window, GlesTexture)>,
}

impl Prepared {
    /// Lend the texture captured for `window`, if there is one.
    ///
    /// Lent rather than taken: with more than one monitor `elements` runs once
    /// per output, and a texture removed by the first one would leave a
    /// deformed window undrawn on every other screen. `GlesTexture` is a
    /// handle, so the clone is a refcount.
    fn texture(&self, window: &Window) -> Option<GlesTexture> {
        self.warps
            .iter()
            .find(|(each, _)| each == window)
            .map(|(_, texture)| texture.clone())
    }
}

/// Capture a texture for every window whose transform is not a rectangle.
///
/// Must run before the backend binds its own buffer; see [`Prepared`].
pub(crate) fn prepare(state: &mut Solium, renderer: &mut GlesRenderer) -> Prepared {
    // Everything here is the compositor's own work, ahead of any output, apart
    // from the tick below — which is entirely Qt's and is measured separately
    // for exactly that reason. See `pacing::Phase`.
    let _prep = crate::pacing::span(crate::pacing::Phase::Prep);
    state.memory_report();
    // Every QML animation in the process, advanced once for this frame --
    // decorations, the cursor, the shell. Whether any scene then has something
    // new to draw is each scene's own answer.
    //
    // Once per *frame* and not once per output: with two monitors, ticking in
    // `elements` would advance every animation twice as fast as the clock, and
    // on monitors of different refresh rates by different amounts.
    {
        let _tick = crate::pacing::span(crate::pacing::Phase::Tick);
        crate::qml::tick(state.clock.now());
    }
    // The shell reads the window list; it changes only when windows do.
    state.publish_windows();

    let mut warps = Vec::new();

    for (pane, window) in state.on_screen() {
        // What this pane wants capturing at, if it wants capturing at all.
        //
        // At *its own monitor's* scale. One frame can span monitors at
        // different scales, and a texture taken at 1x and drawn on a 2x screen
        // is the blur this whole change exists to remove.
        let wanted = window.as_ref().and_then(|window| {
            let outer = state.outer_geometry(window)?;
            let frame = state.drawn(pane, outer);
            (!frame.matrix.is_identity() || frame.deform.is_some()).then(|| state.scale_of(outer))
        });
        let Some((window, scale)) = window.zip(wanted) else {
            // Nothing warped means nothing to keep. A pane holds the texture
            // it was last captured into between frames -- megabytes of it --
            // and there is no later frame on which handing it back gets
            // cheaper, so an overview that warps twenty windows and is then
            // closed would otherwise leave twenty behind for the session. See
            // `offscreen::Scratch`.
            if let Some(pane) = state.panes.get_mut(pane) {
                pane.scratch_mut().release();
            }
            continue;
        };
        if let Some((texture, _size)) =
            crate::offscreen::capture(state, renderer, pane, &window, scale)
        {
            warps.push((window, texture));
        }
    }

    Prepared { warps }
}

/// Everything to draw this frame, topmost first.
///
/// Topmost first is what the damage tracker expects; getting it backwards
/// composites the stack upside down, which looks like a stacking bug rather
/// than an ordering one.
/// The frame around a pane, whatever is inside it.
///
/// Takes the pane's whole `Frame` rather than a rect and an alpha, and that is
/// deliberate: every piece of a window — the client's surface, its popups, its
/// frame, the scene standing in for it — is drawn from the *pane's* transform,
/// so none of them can be given the wrong one or miss one of its parts. The
/// frame did miss the opacity, and a window closing faded away underneath a
/// titlebar that stayed perfectly solid.
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
    frame: present::Frame,
    outer: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    scale: f64,
) {
    let title = state.pane_title(pane);
    let look = crate::decoration::Look {
        title: &title,
        focused: state
            .focused_window()
            .is_some_and(|window| state.panes.id_of(&window) == Some(pane)),
        pointer_inside: state.pointer_over(pane),
    };
    let mut animating = false;
    if let Some(decoration) = state.panes.get_mut(pane).and_then(Pane::decoration_mut) {
        // Already an `Element`: a frame is a memory buffer on the software
        // path and a texture on the GPU one, and which of the two it is is the
        // decoration's own business rather than this function's.
        //
        // And already an answer about whether it is still moving, out of the
        // same call: asking afterwards is asking the flag the draw just spent.
        // See [`Drawn`].
        let drawn = decoration.frame(
            renderer,
            frame.rect,
            outer.size,
            &look,
            frame.opacity,
            scale,
        );
        elements.extend(drawn.element);
        animating = drawn.animating;
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
    let onto = |rect: smithay::utils::Rectangle<f64, smithay::utils::Logical>| {
        smithay::utils::Rectangle::new(rect.loc + shift, rect.size)
    };

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

    // Scripted surfaces at the top layer: above the windows, and below the
    // client surfaces on the same layer -- a real bar covers a scripted one,
    // because the client was installed on purpose.
    elements.extend(scripted(
        state,
        renderer,
        crate::scripted::Layer::Top,
        screen,
        now,
        scale,
    ));

    // Anchored surfaces above the windows: panels, notifications, an overlay.
    // Collected first because the frame is built topmost-first.
    //
    // A layer map's geometry is already in its own output's coordinates, so
    // these are the one thing on this list that must *not* be shifted.
    let output = state.output_for(screen);
    if let Some(output) = output.as_ref() {
        let map = layer_map_for_output(output);
        for surface in map.layers().rev().filter(|layer| layer::is_above(layer)) {
            let Some(geometry) = map.layer_geometry(surface) else {
                continue;
            };
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
    }

    // Scripted surfaces at the overlay layer: above the client bars, below
    // the pointer and the tweaks panel.
    elements.extend(scripted(
        state,
        renderer,
        crate::scripted::Layer::Overlay,
        screen,
        now,
        scale,
    ));

    for (pane, window) in state.on_screen() {
        let Some(global) = state.pane_outer_of(pane) else {
            continue;
        };
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
        // Nothing of the client's left to draw and nothing of ours either:
        // this is a window on its way out. Do not draw half of it.
        if ours && !state.pane_has_scene(pane) {
            continue;
        }

        // The transform is expressed against the *outer* rect — the window
        // including its frame — so the frame scales and moves with the window
        // rather than beside it. Computed in global coordinates, because that
        // is the space a script's target was written in, and moved onto this
        // screen afterwards.
        let mut frame = state.drawn(pane, global);

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
        if !global.overlaps(screen) {
            continue;
        }
        // And within its own monitors, one that has been transformed clean off
        // this screen has nothing to contribute to it.
        //
        // Skipped only when the transform is a plain rectangle. A matrix or a
        // deform can put pixels well outside `frame.rect` — a genie reaches
        // toward a dock — and there is no cheap rect that bounds it, so those
        // are always drawn and the renderer clips.
        if frame.matrix.is_identity()
            && frame.deform.is_none()
            && !frame.rect.overlaps(screen.to_f64())
        {
            continue;
        }
        frame.rect = onto(frame.rect);
        // The frame's share, in drawn pixels: a transform that scaled the
        // window scaled its frame with it.
        let insets = state.insets_of(pane);
        let across = ratio(frame.rect.size.w, outer.size.w);
        let down = ratio(frame.rect.size.h, outer.size.h);
        let (left, top) = (
            f64::from(insets.left) * across,
            f64::from(insets.top) * down,
        );
        let (taken_x, taken_y) = (
            f64::from(insets.horizontal()) * across,
            f64::from(insets.vertical()) * down,
        );

        // Nothing of the application to draw yet, so this window is entirely
        // ours and the scene has all of it, bar included. The frame is built
        // and its room reserved — which is why the window does not change shape
        // when the application arrives — but drawing a bar over a surface that
        // already carries the name says it twice, so that is a setting and it
        // is off.
        if ours {
            if state.loading.decorated {
                chrome(state, renderer, &mut elements, pane, frame, outer, scale);
            }
            scene(state, renderer, &mut elements, pane, frame, now, scale);
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
            scene(state, renderer, &mut elements, pane, frame, now, scale);
        }

        let Some(real) = state.real_geometry(&window) else {
            continue;
        };

        // A transform that is not identity cannot be drawn as a rectangle. The
        // window is rendered flat into a texture first — frame and popups
        // included — and that texture is bent, so the whole window deforms as
        // one thing instead of the client tilting away from its own titlebar.
        if (!frame.matrix.is_identity() || frame.deform.is_some())
            && let Some(mesh) = crate::warp::mesh(frame.rect, frame.matrix, frame.deform, scale)
            && let Some(texture) = prepared.texture(&window)
        {
            elements.push(Element::Warped(crate::warp::Warp::new(
                Id::new(),
                CommitCounter::default(),
                texture,
                mesh,
                frame.opacity,
            )));
            continue;
        }

        // The frame covers the whole window, not a strip of it: whatever it
        // does not draw on is left transparent, and that is what lets a
        // decoration put its bar on any side, or draw a border, or both.
        // Drawn whenever there is a decoration at all, not only when it
        // reserved space: a frame that takes nothing and floats over the
        // window -- a bar that appears on hover, a border that does not push
        // the client around -- is a decoration too.
        chrome(state, renderer, &mut elements, pane, frame, outer, scale);

        // What is left of the drawn rect once the frame has taken its share is
        // the client's.
        let client = present::logical(
            (frame.rect.loc.x + left, frame.rect.loc.y + top),
            (
                (frame.rect.size.w - taken_x).max(1.0),
                (frame.rect.size.h - taken_y).max(1.0),
            ),
        );

        // The surface tree is built as if at its real size and then scaled,
        // which keeps subsurface offsets correct for free.
        let origin = client.loc.to_physical_precise_round(scale);
        let factor = Scale::from((
            ratio(client.size.w, real.size.w),
            ratio(client.size.h, real.size.h),
        ));

        // Popups first: they are above the window they belong to.
        if let Some(surface) = window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone())
        {
            for (popup, offset) in PopupManager::popups_for_surface(&surface) {
                let popup_origin =
                    origin + (offset - popup.geometry().loc).to_physical_precise_round(scale);
                let popup_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        popup.wl_surface(),
                        popup_origin,
                        output_scale,
                        frame.opacity,
                        Kind::Unspecified,
                    );
                elements.extend(popup_elements.into_iter().map(|element| {
                    Element::Window(RescaleRenderElement::from_element(element, origin, factor))
                }));
            }
        }

        // A surface's top-left is not the window's. A client that draws its own
        // decorations puts its drop shadow *outside* the window geometry and
        // tells us so through `set_window_geometry`; drawing the surface at the
        // window's position therefore lands the shadow where the window should
        // be and pushes the window itself down and right by the shadow's width.
        // That is what made Firefox look both misplaced and shadowed. Popups
        // already did this; toplevels did not.
        let surface_origin = origin - window.geometry().loc.to_physical_precise_round(scale);
        let window_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            window.render_elements(renderer, surface_origin, output_scale, frame.opacity);
        elements.extend(window_elements.into_iter().map(|element| {
            Element::Window(RescaleRenderElement::from_element(element, origin, factor))
        }));
    }

    // Scripted surfaces at the bottom layer: under the windows, over the
    // client background surfaces and the wallpaper.
    elements.extend(scripted(
        state,
        renderer,
        crate::scripted::Layer::Bottom,
        screen,
        now,
        scale,
    ));

    // And the ones below: a wallpaper, and anything else a shell puts behind
    // the windows. This output's own, as above.
    if let Some(output) = output.as_ref() {
        let map = layer_map_for_output(output);
        for surface in map.layers().rev().filter(|layer| !layer::is_above(layer)) {
            let Some(geometry) = map.layer_geometry(surface) else {
                continue;
            };
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
    }

    // The bottom of the frame: a wallpaper and anything else declared there.
    // After the client background surfaces above, so a `swaybg` covers this
    // rather than the other way round.
    elements.extend(scripted(
        state,
        renderer,
        crate::scripted::Layer::Background,
        screen,
        now,
        scale,
    ));

    elements
}

/// Everything a script asked the compositor to draw, at one layer.
///
/// One function for the wallpaper, a bar, an overlay and whatever else gets
/// declared — which is the whole point of `scripted.rs`. Nothing in here knows
/// what any of them are for.
fn scripted(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    layer: crate::scripted::Layer,
    screen: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    now: std::time::Duration,
    scale: f64,
) -> Vec<Element> {
    let Some(output) = state.output_for(screen) else {
        return Vec::new();
    };
    let Some(geometry) = state.space.output_geometry(&output) else {
        return Vec::new();
    };
    let primary = state.primary_output();

    // Which of them belong on this screen, decided before anything is
    // borrowed mutably to rasterise it.
    let wanted: Vec<(
        usize,
        smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    )> = state
        .surfaces
        .iter()
        .enumerate()
        .filter(|(_, surface)| surface.layer() == layer)
        .filter_map(|(index, surface)| {
            Some((index, surface.area_on(&output, geometry, primary.as_ref())?))
        })
        .collect();

    let mut drawn = Vec::new();
    let mut animating = false;
    for (index, area) in wanted {
        let Some(surface) = state.surfaces.get_mut(index) else {
            continue;
        };
        let Some(instance) = surface.instance(&output) else {
            continue;
        };
        let painted = instance.element(
            renderer,
            smithay::utils::Rectangle::new(area.loc - screen.loc, area.size),
            now,
            1.0,
            scale,
        );
        drawn.extend(painted.element);
        animating |= painted.animating;
    }
    // Ask for another frame while any of them is still moving, exactly as
    // `chrome` does for a window frame.
    //
    // This did not used to be asked at all, which is the same defect one step
    // further on: a scripted surface got the next frame only when something
    // unrelated damaged the screen. Everything on `Quickshell.SystemClock` is
    // the plain case -- a bar whose clock ticks on a `Timer` -- and it cannot
    // even recover on the next tick, because `qml::tick` is what drains Qt's
    // event queue and it only runs on a frame that is being drawn. No frame,
    // no timer; no timer, no reason for a frame.
    if animating {
        state.redraw = true;
    }
    drawn
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

    match state.pointer.status.clone() {
        CursorImageStatus::Hidden => Vec::new(),
        CursorImageStatus::Surface(surface) => {
            // The hotspot is where *in the image* the pointer actually points,
            // and the client is the only one that knows: drawing at the plain
            // location puts an I-beam's tip a few pixels off the text it is
            // meant to be between.
            let hotspot = with_states(&surface, |states| {
                states
                    .data_map
                    .get::<Mutex<CursorImageAttributes>>()
                    .and_then(|attributes| attributes.lock().ok())
                    .map(|attributes| attributes.hotspot)
                    .unwrap_or_default()
            });
            let origin = (location.to_i32_round() - hotspot).to_physical_precise_round(scale);
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
        CursorImageStatus::Named(_) => state
            .pointer
            .art()
            .and_then(|cursor| cursor.element(renderer, location, scale))
            .into_iter()
            .collect(),
    }
}

/// One window's elements, flat, at the origin and its real size.
///
/// The offscreen pass draws these into a texture so a deformed window is
/// deformed as one thing. Built at the origin because the texture *is* the
/// window's own space; where it lands on screen is the warp's business.
pub(crate) fn flat_window_elements(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    window: &Window,
    scale: f64,
) -> Vec<Element> {
    let mut elements = Vec::new();
    let (Some(real), Some(outer)) = (state.real_geometry(window), state.outer_geometry(window))
    else {
        return elements;
    };
    let insets = state.frame_insets(window);
    let output_scale = Scale::from(scale);

    // The frame, over the whole texture.
    {
        let title = state.window_title(window);
        let look = crate::decoration::Look {
            title: &title,
            focused: state.is_focused(window),
            pointer_inside: state.pointer_inside(window),
        };
        let whole = present::logical(
            (0.0, 0.0),
            (f64::from(outer.size.w), f64::from(outer.size.h)),
        );
        if let Some(id) = state.panes.id_of(window)
            && let Some(decoration) = state.panes.get_mut(id).and_then(Pane::decoration_mut)
        {
            // Fully opaque here: this pass draws the window flat into a
            // texture at its real size, and the warp applies the transform's
            // opacity to the whole texture afterwards. Applying it twice would
            // fade the frame squared.
            let drawn = decoration.frame(renderer, whole, outer.size, &look, 1.0, scale);
            elements.extend(drawn.element);
            // And this pass owes the next frame just as `chrome` does. It is
            // the *only* one that does for a deformed window: this is the draw
            // that spends the scene's flag, and `chrome` skips a window that
            // came through here. A window left tilted by a script is not
            // animating in the compositor's sense and nothing else is asking,
            // so without this a pulse inside a tilted window stops dead.
            if drawn.animating {
                state.redraw = true;
            }
        }
    }

    // The client within it.
    let origin = present::logical(
        (f64::from(insets.left), f64::from(insets.top)),
        (f64::from(real.size.w), f64::from(real.size.h)),
    )
    .loc
    .to_physical_precise_round(scale);

    if let Some(surface) = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone())
    {
        for (popup, offset) in PopupManager::popups_for_surface(&surface) {
            let popup_origin =
                origin + (offset - popup.geometry().loc).to_physical_precise_round(scale);
            let popup_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    popup_origin,
                    output_scale,
                    1.0,
                    Kind::Unspecified,
                );
            elements.extend(popup_elements.into_iter().map(Element::Window2));
        }
    }

    let window_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        window.render_elements(renderer, origin, output_scale, 1.0);
    elements.extend(window_elements.into_iter().map(Element::Window2));
    elements
}

/// Drawn size over real size, guarding the degenerate case.
///
/// A zero-sized window is not drawable, but it is reachable: a client can
/// commit before it has been configured. Scaling by zero would collapse the
/// element and scaling by infinity would take the renderer with it.
fn ratio(drawn: f64, real: i32) -> f64 {
    if real <= 0 || drawn <= 0.0 {
        1.0
    } else {
        drawn / f64::from(real)
    }
}

#[cfg(test)]
mod tests {
    use super::{Drawn, Painted, ratio};

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
}
