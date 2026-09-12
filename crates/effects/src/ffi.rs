//! A C ABI over the deformations, so they can be called from outside Rust.
//!
//! This exists for the preview: the engine is built for `wasm32` and driven
//! from the page, which means the mesh you watch bend in a browser is the one
//! `warp::mesh` builds for a real window. A reimplementation in JavaScript
//! would drift the first time either changed, and the drift would be
//! invisible -- the page would still bend something plausible.
//!
//! A grid is written into a fixed scratch buffer rather than allocated, the
//! same way `solium_layout::ffi` publishes its rectangles: the crate has no
//! dependencies and wants none, and a wasm module that never allocates is a
//! wasm module with nothing to leak.

use crate::{Axis, Deform, Rect};

/// The most points a grid will report, as `x, y` pairs.
///
/// Bigger than any grid [`Deform::segments`] asks for today -- a genie is 49
/// by 9 -- with room for an effect that wants a finer one in both directions
/// before this has to be thought about again.
pub const CAPACITY: usize = 64 * 64;

static mut SCRATCH: [f64; CAPACITY * 2] = [0.0; CAPACITY * 2];

/// Where the last grid was written.
#[expect(unsafe_code, reason = "exporting a C symbol and its scratch buffer")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_effect_scratch() -> *const f64 {
    &raw const SCRATCH as *const f64
}

#[expect(unsafe_code, reason = "exporting a C symbol")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_effect_capacity() -> u32 {
    CAPACITY as u32
}

/// How many effects there are, so a caller can enumerate them.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_effect_count() -> u32 {
    Deform::all().len() as u32
}

/// How many axes there are.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_axis_count() -> u32 {
    Axis::all().len() as u32
}

/// The effect at `index` in [`Deform::all`], with the parameters given.
///
/// Out of range falls back to the first effect rather than refusing: the page
/// can be older than the engine embedded in it, and a preview that renders
/// the wrong effect is a better failure than one that renders nothing.
fn effect(index: u32, axis: u32, progress: f64, spread: f64) -> Deform {
    let axis = Axis::all()
        .get(axis as usize)
        .map_or_else(Axis::default, |(_, axis)| *axis);
    // Destructured rather than indexed: the array's length is in its type, so
    // "there is a first effect" is checked by the compiler and not by a bound.
    let [(_, first), ..] = Deform::all();
    match Deform::all()
        .get(index as usize)
        .map_or(first, |(_, effect)| *effect)
    {
        Deform::Genie { .. } => Deform::Genie {
            progress: crate::number(progress),
            spread: crate::number(spread),
            axis,
        },
    }
}

/// How many columns the grid for this effect has. Points across is one more.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_effect_columns(index: u32, axis: u32) -> u32 {
    effect(index, axis, 0.0, 0.0).segments().0
}

/// How many rows the grid for this effect has. Points down is one more.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_effect_rows(index: u32, axis: u32) -> u32 {
    effect(index, axis, 0.0, 0.0).segments().1
}

/// Deform a window into a target and write the whole grid out.
///
/// Returns how many points were written. They are in row-major order --
/// `columns + 1` across, `rows + 1` down -- which is the order `warp::mesh`
/// walks, so a page drawing quads out of this is drawing the compositor's
/// own cells.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_effect_grid(
    index: u32,
    axis: u32,
    progress: f64,
    spread: f64,
    from_x: f64,
    from_y: f64,
    from_w: f64,
    from_h: f64,
    to_x: f64,
    to_y: f64,
    to_w: f64,
    to_h: f64,
) -> u32 {
    let deform = effect(index, axis, progress, spread);
    let from = Rect::new(from_x, from_y, from_w, from_h);
    let to = Rect::new(to_x, to_y, to_w, to_h);
    let (columns, rows) = deform.segments();

    let mut written = 0_usize;
    for row in 0..=rows {
        let v = f64::from(row) / f64::from(rows);
        for column in 0..=columns {
            if written >= CAPACITY {
                break;
            }
            let u = f64::from(column) / f64::from(columns);
            let (x, y) = deform.place(from, to, u, v);
            publish(written, x, y);
            written += 1;
        }
    }
    written as u32
}

/// Put one point into the scratch buffer.
#[expect(unsafe_code, reason = "writing the scratch buffer the caller reads")]
fn publish(index: usize, x: f64, y: f64) {
    if index >= CAPACITY {
        return;
    }
    let scratch = &raw mut SCRATCH;
    // SAFETY: single-threaded by construction -- this is called from a wasm
    // module with one thread, never from the compositor, and `index` is
    // checked against the buffer's length immediately above. Written through a
    // raw pointer rather than a `&mut` to the static, so no reference to it
    // exists that a concurrent read could alias.
    unsafe {
        (*scratch)[index * 2] = x;
        (*scratch)[index * 2 + 1] = y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page must not be able to see a different answer than the
    /// compositor does.
    #[expect(unsafe_code, reason = "reading the scratch buffer the page reads")]
    #[test]
    fn the_exported_surface_agrees_with_the_engine() {
        let from = Rect::new(0.0, 0.0, 800.0, 600.0);
        let to = Rect::new(400.0, 900.0, 64.0, 32.0);

        for index in 0..solium_effect_count() {
            for axis in 0..solium_axis_count() {
                let deform = effect(index, axis, 0.4, 1.2);
                let (columns, rows) = deform.segments();
                assert_eq!(solium_effect_columns(index, axis), columns);
                assert_eq!(solium_effect_rows(index, axis), rows);

                let written = solium_effect_grid(
                    index, axis, 0.4, 1.2, from.x, from.y, from.w, from.h, to.x, to.y, to.w, to.h,
                );
                assert_eq!(written, (columns + 1) * (rows + 1));

                // Read back the way the page does, and compare against the
                // engine called directly.
                let base = solium_effect_scratch();
                for row in 0..=rows {
                    for column in 0..=columns {
                        let at = (row * (columns + 1) + column) as usize;
                        let (u, v) = (
                            f64::from(column) / f64::from(columns),
                            f64::from(row) / f64::from(rows),
                        );
                        let (x, y) = deform.place(from, to, u, v);
                        // SAFETY: `at` is below `written`, which the assertion
                        // above pins to the grid this loop walks, and the
                        // buffer is not written while it is read.
                        let (got_x, got_y) = unsafe { (*base.add(at * 2), *base.add(at * 2 + 1)) };
                        assert!((got_x - x).abs() < f64::EPSILON, "point {at} x");
                        assert!((got_y - y).abs() < f64::EPSILON, "point {at} y");
                    }
                }
            }
        }
    }

    #[test]
    fn an_out_of_range_effect_falls_back_rather_than_panicking() {
        // Reachable from a page that is out of date with the engine.
        assert_eq!(effect(9999, 9999, 0.5, 1.0), effect(0, 0, 0.5, 1.0));
    }

    /// A grid never runs past the buffer the page reads out of.
    #[test]
    fn the_grid_fits_the_buffer_it_is_published_in() {
        for index in 0..solium_effect_count() {
            for axis in 0..solium_axis_count() {
                let (columns, rows) = effect(index, axis, 0.0, 0.0).segments();
                assert!(
                    ((columns + 1) * (rows + 1)) as usize <= CAPACITY,
                    "{}x{} points does not fit {CAPACITY}",
                    columns + 1,
                    rows + 1
                );
            }
        }
    }
}
