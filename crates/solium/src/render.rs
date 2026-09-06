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
        ImportAll, ImportMem, Renderer,
        element::{
            AsRenderElements, Kind,
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::RescaleRenderElement,
        },
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
    /// drawn from QML.
    pub(crate) Element<R> where R: ImportAll + ImportMem;
    Window = RescaleRenderElement<WaylandSurfaceRenderElement<R>>,
    Chrome = MemoryRenderBufferRenderElement<R>,
}

/// Everything to draw this frame, topmost first.
///
/// Topmost first is what the damage tracker expects; getting it backwards
/// composites the stack upside down, which looks like a stacking bug rather
/// than an ordering one.
pub(crate) fn elements<R>(state: &mut Solium, renderer: &mut R, scale: f64) -> Vec<Element<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let now = state.clock.now();
    let output_scale = Scale::from(scale);
    let mut elements = Vec::new();

    // The pointer, above everything — including anything a shell anchors on
    // top. Nothing else draws it, so leaving it out is not a missing detail:
    // it is a session where the mouse appears not to work.
    elements.extend(cursor(state, renderer, output_scale, scale, now));

    // The shell reads the window list; it changes only when windows do.
    state.publish_windows();

    // The shell's own surfaces, above the windows it sits over. Drawn from the
    // same QML engine as the window frames, which is what lets an icon here and
    // a window there be interpolated between.
    if let Some(area) = state.dock_area()
        && let Some(dock) = state.dock.as_mut()
        && let Some(element) = dock.element(renderer, area, now)
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
            let layer_elements: Vec<WaylandSurfaceRenderElement<R>> =
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
                let popup_elements: Vec<WaylandSurfaceRenderElement<R>> =
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
        let window_elements: Vec<WaylandSurfaceRenderElement<R>> =
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
            let layer_elements: Vec<WaylandSurfaceRenderElement<R>> =
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
fn cursor<R>(
    state: &mut Solium,
    renderer: &mut R,
    output_scale: Scale<f64>,
    scale: f64,
    now: std::time::Duration,
) -> Vec<Element<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
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
            let surface_elements: Vec<WaylandSurfaceRenderElement<R>> =
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
