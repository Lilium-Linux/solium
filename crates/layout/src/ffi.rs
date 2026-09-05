//! A C ABI over the arrangements, so the preview can call them.
//!
//! Results are written into a fixed scratch buffer rather than allocated: the
//! crate has no dependencies and wants none, and a wasm module that never
//! allocates is a wasm module with nothing to leak. The caller reads the
//! rectangles straight out of the module's memory.

use crate::{Rect, Settings};

/// The most windows an arrangement will report. Beyond this the extras are not
/// laid out — a preview with sixty-five windows is not a case worth carrying an
/// allocator for.
pub const CAPACITY: usize = 64;

/// Four `f64` per rectangle: x, y, w, h.
static mut SCRATCH: [f64; CAPACITY * 4] = [0.0; CAPACITY * 4];

/// Where the last arrangement was written.
#[expect(unsafe_code, reason = "exporting a C symbol and its scratch buffer")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_layout_scratch() -> *const f64 {
    &raw const SCRATCH as *const f64
}

#[must_use]
pub const fn capacity() -> usize {
    CAPACITY
}

#[expect(unsafe_code, reason = "exporting a C symbol")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_layout_capacity() -> u32 {
    CAPACITY as u32
}

/// Copy `slots` into the scratch buffer and return how many were written.
#[expect(unsafe_code, reason = "writing the scratch buffer the caller reads")]
fn publish(slots: &[Rect]) -> u32 {
    let written = slots.len().min(CAPACITY);
    let scratch = &raw mut SCRATCH;
    for (index, slot) in slots.iter().take(written).enumerate() {
        // SAFETY: single-threaded by construction — this crate is called from
        // the compositor's render thread or from a wasm module, never both, and
        // `written` is clamped to the buffer's length above. Written through a
        // raw pointer rather than a `&mut` to the static, so no reference to it
        // exists that a concurrent read could alias.
        unsafe {
            (*scratch)[index * 4] = slot.x;
            (*scratch)[index * 4 + 1] = slot.y;
            (*scratch)[index * 4 + 2] = slot.w;
            (*scratch)[index * 4 + 3] = slot.h;
        }
    }
    written as u32
}

fn settings(gap: f64, ratio: f64, column: f64, padding: f64) -> Settings {
    Settings {
        gap,
        ratio,
        column,
        padding,
        // The preview drives the arrangements it has controls for; dwindle
        // takes its split from the default until the page grows a slider.
        ..Settings::default()
    }
}

#[expect(unsafe_code, reason = "exporting a C symbol")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_layout_master_stack(
    count: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    gap: f64,
    ratio: f64,
) -> u32 {
    let slots = crate::master_stack(
        count as usize,
        Rect::new(x, y, w, h),
        settings(gap, ratio, 0.44, 24.0),
    );
    publish(&slots)
}

#[expect(unsafe_code, reason = "exporting a C symbol")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_layout_scrolling(
    count: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    gap: f64,
    column: f64,
    offset: f64,
) -> u32 {
    let slots = crate::scrolling(
        count as usize,
        Rect::new(x, y, w, h),
        settings(gap, 0.6, column, 24.0),
        offset,
    );
    publish(&slots)
}

/// The grid, given each window's own rectangle in the scratch buffer.
///
/// Written in, read out of the same place: the caller fills the scratch with
/// `count` rectangles, calls this, and reads the arrangement back from it.
#[expect(unsafe_code, reason = "exporting a C symbol and reading its scratch")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_layout_grid(
    count: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    padding: f64,
) -> u32 {
    let count = (count as usize).min(CAPACITY);
    let scratch = &raw const SCRATCH;
    let sizes: Vec<Rect> = (0..count)
        .map(|index| {
            // SAFETY: as in `publish`, and `count` is clamped to the buffer.
            unsafe {
                Rect::new(
                    (*scratch)[index * 4],
                    (*scratch)[index * 4 + 1],
                    (*scratch)[index * 4 + 2],
                    (*scratch)[index * 4 + 3],
                )
            }
        })
        .collect();

    let slots = crate::grid(
        &sizes,
        Rect::new(x, y, w, h),
        settings(12.0, 0.6, 0.44, padding),
    );
    publish(&slots)
}

#[expect(unsafe_code, reason = "exporting a C symbol")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_layout_scroll_to(
    index: u32,
    count: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    gap: f64,
    column: f64,
    offset: f64,
) -> f64 {
    crate::scroll_to(
        index as usize,
        count as usize,
        Rect::new(x, y, w, h),
        settings(gap, 0.6, column, 24.0),
        offset,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(unsafe_code, reason = "reading the scratch the call just wrote")]
    fn the_exported_arrangement_matches_the_engine() {
        let written = solium_layout_master_stack(3, 0.0, 34.0, 1600.0, 866.0, 12.0, 0.6);
        assert_eq!(written, 3);

        let direct =
            crate::master_stack(3, Rect::new(0.0, 34.0, 1600.0, 866.0), Settings::default());
        let scratch = &raw const SCRATCH;
        for (index, slot) in direct.iter().enumerate() {
            // SAFETY: single-threaded test, reading what the call above wrote.
            unsafe {
                assert!(((*scratch)[index * 4] - slot.x).abs() < f64::EPSILON);
                assert!(((*scratch)[index * 4 + 2] - slot.w).abs() < f64::EPSILON);
            }
        }
    }

    #[test]
    fn more_windows_than_the_buffer_holds_are_dropped_not_written_past() {
        let written = solium_layout_master_stack(500, 0.0, 0.0, 1600.0, 900.0, 12.0, 0.6);
        assert_eq!(written as usize, CAPACITY);
    }
}
