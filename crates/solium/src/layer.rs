//! Layer surfaces: how a shell that runs as its own program attaches to the
//! compositor.
//!
//! A bar, a dock, a notification area and a wallpaper can be **ordinary
//! clients** that anchor themselves to an edge of an output and say how much
//! room they need; the compositor honours that and keeps windows out of it.
//! The protocol is `wlr-layer-shell`, so any existing panel works and the shell
//! can be written in whatever its author likes. The other way in is a shell
//! hosted inside the compositor as configuration, `shell = { scene = ... }`
//! (`lua/shell.lua`; `script::tests::the_shell_scene_is_read_from_the_configuration`).
//! `docs/shell-boundary.md` has both.
//!
//! ## Why the compositor has no bar of its own
//!
//! It had one, briefly, as the vehicle for getting QML rendering in-process — and
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
//! — disappear rather than get answered. A shell hosted in-process today is the
//! answer to that question and not the old bar back: it is whatever the
//! configuration names, and the shipped configuration names none
//! (`script::tests::the_shipped_configuration_hosts_no_shell_with_preview_off`).
//!
//! ## What the work area is now
//!
//! Not a constant. Windows are placed in whatever is left after every anchored
//! surface has taken its exclusive zone, which is a number the shell chooses
//! and can change at runtime, and every hosted surface its `reserve`
//! (`state::tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`).
//! `Solium::work_area` reads it.

use smithay::{
    desktop::{LayerMap, LayerSurface, layer_map_for_output},
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

/// One layer's surfaces on a monitor, topmost first: the last one mapped is
/// on top.
///
/// **The order within a layer, for the renderer and the hit test both**:
/// `render::stacked` draws a layer's surfaces in this order and
/// [`surface_under`] asks them in it. Which layer is over which is
/// `crate::stack`'s. `an_overlay_mapped_before_a_bar_is_drawn_over_it_and_takes_the_press`.
pub(crate) fn on(
    map: &LayerMap,
    layer: crate::scripted::Layer,
) -> impl Iterator<Item = &LayerSurface> {
    map.layers_on(wlr(layer)).rev()
}

/// The protocol's name for a layer.
const fn wlr(layer: crate::scripted::Layer) -> Layer {
    match layer {
        crate::scripted::Layer::Background => Layer::Background,
        crate::scripted::Layer::Bottom => Layer::Bottom,
        crate::scripted::Layer::Top => Layer::Top,
        crate::scripted::Layer::Overlay => Layer::Overlay,
    }
}

/// The surface one layer has at a point, with its origin, topmost first.
///
/// `point` is in the output's own coordinates and so is the origin that comes
/// back; the caller adds the output's position to get either into the
/// compositor's space. The origin is the surface's top-left corner, so that
/// `point - origin` is the point within the surface -- which is the only
/// number the client is ever told.
///
/// One layer, and the caller walks them in `crate::stack`'s order: asking
/// `overlay` and then `top` here was the hit test's own copy of it.
pub(crate) fn surface_under(
    output: &Output,
    layer: crate::scripted::Layer,
    point: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let map = layer_map_for_output(output);
    on(&map, layer).find_map(|surface| {
        let geometry = map.layer_geometry(surface)?;
        surface
            .surface_under(
                point - geometry.loc.to_f64(),
                smithay::desktop::WindowSurfaceType::ALL,
            )
            // The subsurface's offset is within the layer surface, so the
            // layer's own position has to be added back. Returning `point -
            // offset` here instead -- the surface-local point in place of the
            // origin -- told every bar and dock that the pointer was somewhere
            // it was not, and told it a *different* wrong place for each
            // position of the cursor.
            .map(|(surface, offset)| (surface, (geometry.loc + offset).to_f64()))
    })
}

/// Tell every bar and dock on a monitor that it is going away.
///
/// The protocol is explicit: when an output is destroyed, its layer surfaces
/// are closed. Skipping it leaves a bar holding a surface for a screen that no
/// longer exists, waiting for a configure that will never come -- and when the
/// monitor is plugged back in, the bar does not reappear, because as far as it
/// knows it never left.
///
/// The map itself needs no clearing: it lives in the output's own user data
/// and goes when the output does.
pub(crate) fn close_all(output: &Output) {
    let map = layer_map_for_output(output);
    for layer in map.layers() {
        layer.layer_surface().send_close();
    }
}
