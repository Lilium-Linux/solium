//! Modes: overview, and eventually the app switcher, peek and the genie.
//!
//! Overview lives here in Rust as the *proof* that the transform layer can
//! express a mode at all. #17 moves it to `overview.lua`, and when it moves,
//! nothing outside this file should have to change. If moving it needs new
//! Rust, the script API is missing something and that is the bug — see
//! `docs/architecture.md`.
//!
//! Note what this file does not contain: no renderer, no layout arithmetic
//! written back into the space, no per-mode configuration. It reads geometry,
//! sets targets, and lets the one clock animate them.

use std::time::Duration;

use smithay::{
    desktop::Window,
    utils::{Logical, Rectangle},
};

use crate::{
    present::{self, Easing, Frame, logical},
    state::Solium,
};

/// Entering is slower than leaving on purpose: arriving somewhere new wants to
/// be readable, going back to what you already know wants to be quick.
const ENTER: Duration = Duration::from_millis(260);
const LEAVE: Duration = Duration::from_millis(200);

/// Gap around each thumbnail, so windows read as separate cards.
const PADDING: f64 = 24.0;

pub(crate) fn toggle_overview(state: &mut Solium) {
    if state.overview {
        leave_overview(state);
    } else {
        enter_overview(state);
    }
}

/// Scale every window down onto a grid.
pub(crate) fn enter_overview(state: &mut Solium) {
    let Some(output) = state.work_area() else {
        tracing::warn!("no output, refusing to enter overview");
        return;
    };

    let windows: Vec<Window> = state.space.elements().cloned().collect();
    if windows.is_empty() {
        // Not an error, and specifically not a state change: entering an empty
        // overview would leave a mode active with nothing to leave it with.
        tracing::debug!("no windows, overview does nothing");
        return;
    }

    let now = state.clock.now();
    let columns = columns_for(windows.len());
    let rows = windows.len().div_ceil(columns);

    for (index, window) in windows.iter().enumerate() {
        let Some(real) = state.real_geometry(window) else {
            continue;
        };
        let target = fit(real, cell(output, columns, rows, index));
        present::present(window, real, target, now, ENTER, Easing::OutCubic);
    }

    state.overview = true;
    tracing::info!(windows = windows.len(), "overview entered");
}

/// Put every window back exactly where it was.
///
/// "Exactly" is free rather than careful: the layout was never touched, so
/// leaving is just animating the transform back to real geometry and dropping
/// it.
pub(crate) fn leave_overview(state: &mut Solium) {
    let now = state.clock.now();
    let windows: Vec<Window> = state.space.elements().cloned().collect();

    for window in &windows {
        let Some(real) = state.real_geometry(window) else {
            continue;
        };
        present::clear(window, real, now, LEAVE, Easing::OutCubic);
    }

    state.overview = false;
    tracing::info!("overview left");
}

/// A grid that stays close to square, so windows are as large as they can be.
fn columns_for(count: usize) -> usize {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a window count large enough to lose precision is not reachable"
    )]
    let columns = (count as f64).sqrt().ceil() as usize;
    columns.max(1)
}

/// The area one window gets, before its aspect ratio is taken into account.
fn cell(
    output: Rectangle<i32, Logical>,
    columns: usize,
    rows: usize,
    index: usize,
) -> Rectangle<f64, Logical> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "grid dimensions are small; precision loss is not reachable"
    )]
    let (columns_f, rows_f) = (columns as f64, rows.max(1) as f64);
    let width = f64::from(output.size.w) / columns_f;
    let height = f64::from(output.size.h) / rows_f;

    #[expect(
        clippy::cast_precision_loss,
        reason = "grid dimensions are small; precision loss is not reachable"
    )]
    let (column, row) = ((index % columns) as f64, (index / columns) as f64);

    logical(
        (
            f64::from(output.loc.x) + column * width + PADDING,
            f64::from(output.loc.y) + row * height + PADDING,
        ),
        (
            (width - PADDING * 2.0).max(1.0),
            (height - PADDING * 2.0).max(1.0),
        ),
    )
}

/// Fit a window into a cell, keeping its aspect ratio and never enlarging it.
///
/// Never enlarging matters: a small window blown up to fill a grid cell in
/// overview looks like a different window.
fn fit(real: Rectangle<i32, Logical>, cell: Rectangle<f64, Logical>) -> Frame {
    let (real_width, real_height) = (f64::from(real.size.w), f64::from(real.size.h));
    if real_width <= 0.0 || real_height <= 0.0 {
        return Frame {
            rect: cell,
            opacity: 1.0,
        };
    }

    let scale = (cell.size.w / real_width)
        .min(cell.size.h / real_height)
        .min(1.0);
    let (width, height) = (real_width * scale, real_height * scale);

    Frame {
        rect: logical(
            (
                cell.loc.x + (cell.size.w - width) / 2.0,
                cell.loc.y + (cell.size.h - height) / 2.0,
            ),
            (width, height),
        ),
        opacity: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_stays_close_to_square() {
        assert_eq!(columns_for(1), 1);
        assert_eq!(columns_for(2), 2);
        assert_eq!(columns_for(4), 2);
        assert_eq!(columns_for(5), 3);
        assert_eq!(columns_for(9), 3);
        // Never zero, or the cell arithmetic divides by it.
        assert_eq!(columns_for(0), 1);
    }

    #[test]
    fn cells_tile_the_output_without_overlapping() {
        let output = Rectangle::new((0, 0).into(), (1600, 900).into());
        let first = cell(output, 2, 2, 0);
        let second = cell(output, 2, 2, 1);
        let third = cell(output, 2, 2, 2);

        assert!(
            first.loc.x + first.size.w <= second.loc.x,
            "columns must not overlap"
        );
        assert!(
            first.loc.y + first.size.h <= third.loc.y,
            "rows must not overlap"
        );
    }

    #[test]
    fn fitting_preserves_aspect_ratio() {
        // A 2:1 window in a square cell keeps 2:1.
        let real = Rectangle::new((0, 0).into(), (800, 400).into());
        let cell = logical((0.0, 0.0), (200.0, 200.0));
        let fitted = fit(real, cell);
        assert!((fitted.rect.size.w / fitted.rect.size.h - 2.0).abs() < 1e-9);
        // Centred in the cell.
        assert!((fitted.rect.loc.y - 50.0).abs() < 1e-9);
    }

    #[test]
    fn fitting_never_enlarges() {
        let real = Rectangle::new((0, 0).into(), (100, 100).into());
        let cell = logical((0.0, 0.0), (800.0, 800.0));
        let fitted = fit(real, cell);
        assert!((fitted.rect.size.w - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_degenerate_window_does_not_divide_by_zero() {
        let real = Rectangle::new((0, 0).into(), (0, 0).into());
        let cell = logical((0.0, 0.0), (100.0, 100.0));
        assert_eq!(fit(real, cell).rect, cell);
    }
}
