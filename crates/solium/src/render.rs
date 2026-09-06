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

use crate::{layer, present, state::Solium};

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
    /// Hand over the texture captured for `window`, if there is one.
    fn take(&mut self, window: &Window) -> Option<GlesTexture> {
        let at = self.warps.iter().position(|(each, _)| each == window)?;
        Some(self.warps.swap_remove(at).1)
    }
}

/// Capture a texture for every window whose transform is not a rectangle.
///
/// Must run before the backend binds its own buffer; see [`Prepared`].
pub(crate) fn prepare(state: &mut Solium, renderer: &mut GlesRenderer, scale: f64) -> Prepared {
    let now = state.clock.now();
    let windows: Vec<Window> = state.space.elements().rev().cloned().collect();
    let mut warps = Vec::new();

    for window in windows {
        let Some(outer) = state.outer_geometry(&window) else {
            continue;
        };
        if present::frame(&window, outer, now).matrix.is_identity() {
            continue;
        }
        if let Some((texture, _size)) =
            crate::offscreen::capture(state, renderer, &window, now, scale)
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
pub(crate) fn elements(
    state: &mut Solium,
    renderer: &mut GlesRenderer,
    scale: f64,
    prepared: &mut Prepared,
) -> Vec<Element> {
    let now = state.clock.now();
    let output_scale = Scale::from(scale);
    let mut elements = Vec::new();

    // The pointer, above everything — including anything a shell anchors on
    // top. Nothing else draws it, so leaving it out is not a missing detail:
    // it is a session where the mouse appears not to work.
    elements.extend(cursor(state, renderer, output_scale, scale, now));

    // The shell reads the window list; it changes only when windows do.
    state.publish_windows();

    // The shell, when one is hosted: above the windows, below the pointer.
    state.publish_windows();
    if let Some(area) = state.work_area()
        && let Some(shell) = state.shell()
        && let Some(element) = shell.element(renderer, area, now)
    {
        elements.push(Element::Chrome(element));
    }

    // Anchored surfaces above the windows: panels, notifications, an overlay.
    // Collected first because the frame is built topmost-first.
    let output = state.space.outputs().next().cloned();
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

    // Collected first because the loop needs `&mut state` to render frames.
    // `Window` is a handle, so this is a few pointer copies.
    let windows: Vec<Window> = state.space.elements().rev().cloned().collect();

    for window in windows {
        let (Some(real), Some(outer)) =
            (state.real_geometry(&window), state.outer_geometry(&window))
        else {
            continue;
        };

        // The transform is expressed against the *outer* rect — the window
        // including its frame — so the frame scales and moves with the window
        // rather than beside it.
        let frame = present::frame(&window, outer, now);
        let inset = f64::from(state.frame_inset(&window)) * ratio(frame.rect.size.h, outer.size.h);

        // A transform that is not identity cannot be drawn as a rectangle. The
        // window is rendered flat into a texture first — frame and popups
        // included — and that texture is bent, so the whole window deforms as
        // one thing instead of the client tilting away from its own titlebar.
        if !frame.matrix.is_identity()
            && let Some(corners) = crate::warp::project_quad(frame.rect, frame.matrix, scale)
            && let Some(texture) = prepared.take(&window)
        {
            elements.push(Element::Warped(crate::warp::Warp::new(
                Id::new(),
                CommitCounter::default(),
                texture,
                corners,
                frame.opacity,
            )));
            continue;
        }

        if inset >= 1.0 {
            let title = state.window_title(&window);
            let focused = state.is_focused(&window);
            let bar = present::logical(
                (frame.rect.loc.x, frame.rect.loc.y),
                (frame.rect.size.w, inset),
            );
            if let Some(id) = state.toplevel_id(&window)
                && let Some(decoration) = state.decorations.get_mut(&id)
                && let Some(element) =
                    decoration.frame(renderer, bar, real.size.w, &title, focused, now)
            {
                elements.push(Element::Chrome(element));
            }
        }

        // What is left of the drawn rect once the frame has taken its share is
        // the client's, which is what "the frame reserves its height" means.
        let client = present::logical(
            (frame.rect.loc.x, frame.rect.loc.y + inset),
            (frame.rect.size.w, (frame.rect.size.h - inset).max(1.0)),
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

    // And the ones below: a wallpaper, and anything else a shell puts behind
    // the windows.
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

    elements
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
    now: std::time::Duration,
) -> Vec<Element> {
    let Some(pointer) = state.seat.get_pointer() else {
        return Vec::new();
    };
    let location = pointer.current_location();

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
        CursorImageStatus::Named(_) => state
            .pointer
            .art()
            .and_then(|cursor| cursor.element(renderer, location, now))
            .map(Element::Chrome)
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
    now: std::time::Duration,
    scale: f64,
) -> Vec<Element> {
    let mut elements = Vec::new();
    let (Some(real), Some(outer)) = (state.real_geometry(window), state.outer_geometry(window))
    else {
        return elements;
    };
    let inset = f64::from(state.frame_inset(window));
    let output_scale = Scale::from(scale);

    // The frame, at the top of the texture.
    if inset >= 1.0 {
        let title = state.window_title(window);
        let focused = state.is_focused(window);
        let bar = present::logical((0.0, 0.0), (f64::from(outer.size.w), inset));
        if let Some(id) = state.toplevel_id(window)
            && let Some(decoration) = state.decorations.get_mut(&id)
            && let Some(element) =
                decoration.frame(renderer, bar, real.size.w, &title, focused, now)
        {
            elements.push(Element::Chrome(element));
        }
    }

    // The client below it.
    let origin = present::logical(
        (0.0, inset),
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
    use super::ratio;

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
