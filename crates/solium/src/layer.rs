//! Layer surfaces: how a shell attaches to the compositor.
//!
//! This is the boundary between Solium and Lilium. A bar, a dock, a
//! notification area and a wallpaper are **ordinary clients** that anchor
//! themselves to an edge of an output and say how much room they need; the
//! compositor honours that and keeps windows out of it. The protocol is
//! `wlr-layer-shell`, so any existing panel works and the shell can be written
//! in whatever its author likes.
//!
//! ## Why the compositor does not draw the bar
//!
//! It did, briefly, as the vehicle for getting QML rendering in-process — and
//! that was the wrong home for it. The rule the project already had says it
//! plainly:
//!
//! > In-process for anything **window-coupled**. Decorations and mode overlays
//! > run inside the compositor. A dock or bar is a separate process that
//! > publishes geometry, never pixels.
//!
//! A titlebar has to move in the same frame as its window or the two visibly
//! come apart, so it belongs here. A bar does not: it sits still, it needs no
//! window's geometry, and everything it displays — the clock, the tray, the
//! workspace list — is the shell's business. Building it in meant the
//! compositor owned a design, a font stack and a layout it had no reason to,
//! and it made the interesting question — how does a *replaceable* shell attach
//! — disappear rather than get answered.
//!
//! ## What the work area is now
//!
//! Not a constant. Windows are placed in whatever is left after every anchored
//! surface has taken its exclusive zone, which is a number the shell chooses
//! and can change at runtime. `Solium::work_area` reads it.

use smithay::{
    desktop::{LayerSurface, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::shell::wlr_layer::Layer,
};

/// Place every anchored surface and return whether anything moved.
///
/// Called when a layer surface appears, is reconfigured or goes away. The
/// result matters: a changed exclusive zone changes the work area, and windows
/// placed against the old one are now in the wrong place.
pub(crate) fn arrange(output: &Output) -> bool {
    layer_map_for_output(output).arrange()
}

/// What is left of an output once anchored surfaces have taken their share.
pub(crate) fn work_area(output: &Output) -> Rectangle<i32, Logical> {
    layer_map_for_output(output).non_exclusive_zone()
}

/// The layer surface drawn at a point, with its origin, searched top down.
///
/// Only the layers above windows are considered: a click on the wallpaper
/// belongs to whatever is above it, not to the wallpaper.
pub(crate) fn surface_under(
    output: &Output,
    point: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let map = layer_map_for_output(output);
    let layer = map
        .layer_under(Layer::Overlay, point)
        .or_else(|| map.layer_under(Layer::Top, point))?;
    let geometry = map.layer_geometry(layer)?;
    layer
        .surface_under(
            point - geometry.loc.to_f64(),
            smithay::desktop::WindowSurfaceType::ALL,
        )
        .map(|(surface, offset)| (surface, point - offset.to_f64()))
}

/// Whether a layer wants to be drawn above windows.
pub(crate) fn is_above(layer: &LayerSurface) -> bool {
    matches!(layer.layer(), Layer::Top | Layer::Overlay)
}
