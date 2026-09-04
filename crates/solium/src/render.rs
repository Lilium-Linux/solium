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
    desktop::{PopupManager, Window},
    utils::Scale,
};

use crate::{present, state::Solium};

render_elements! {
    /// Everything Solium can draw.
    ///
    /// `Window` is a client's surface, placed wherever its presentation says.
    /// `Bar` is a texture the compositor rendered itself — today that is the
    /// QML top bar, tomorrow window decorations from the same scene graph.
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

        let window_elements: Vec<WaylandSurfaceRenderElement<R>> =
            window.render_elements(renderer, origin, output_scale, frame.opacity);
        elements.extend(window_elements.into_iter().map(|element| {
            Element::Window(RescaleRenderElement::from_element(element, origin, factor))
        }));
    }

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
