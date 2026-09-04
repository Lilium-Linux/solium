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
        ImportAll, Renderer,
        element::{
            AsRenderElements, Kind,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::RescaleRenderElement,
        },
    },
    desktop::PopupManager,
    utils::Scale,
};

use crate::{present, state::Solium};

/// A window's surface, drawn wherever its presentation says.
pub(crate) type Element<R> = RescaleRenderElement<WaylandSurfaceRenderElement<R>>;

/// Everything to draw this frame, topmost first.
///
/// Topmost first is what the damage tracker expects; getting it backwards
/// composites the stack upside down, which looks like a stacking bug rather
/// than an ordering one.
pub(crate) fn elements<R>(state: &Solium, renderer: &mut R, scale: f64) -> Vec<Element<R>>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + 'static,
{
    let now = state.clock.now();
    let output_scale = Scale::from(scale);
    let mut elements = Vec::new();

    for window in state.space.elements().rev() {
        let Some(real) = state.real_geometry(window) else {
            continue;
        };
        let frame = present::frame(window, real, now);

        // The transform is expressed as a scale about the drawn origin, so the
        // surface tree is built as if it were at its real size and then scaled.
        // That keeps subsurface offsets correct for free.
        let origin = frame.rect.loc.to_physical_precise_round(scale);
        let factor = Scale::from((
            ratio(frame.rect.size.w, real.size.w),
            ratio(frame.rect.size.h, real.size.h),
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
                elements.extend(
                    popup_elements
                        .into_iter()
                        .map(|element| RescaleRenderElement::from_element(element, origin, factor)),
                );
            }
        }

        let window_elements: Vec<WaylandSurfaceRenderElement<R>> =
            window.render_elements(renderer, origin, output_scale, frame.opacity);
        elements.extend(
            window_elements
                .into_iter()
                .map(|element| RescaleRenderElement::from_element(element, origin, factor)),
        );
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
