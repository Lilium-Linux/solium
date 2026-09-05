//! Solium's window arrangements.
//!
//! Where each window goes, given how many there are and how much room. Nothing
//! here knows what a window *is* — no surfaces, no compositor, no Wayland — so
//! the same code arranges real windows and arranges the boxes in the preview
//! page. That is the point: a layout you can only judge by opening four
//! terminals is a layout nobody tunes, and a layout reimplemented in the
//! preview is a layout the preview is lying about.
//!
//! Scripts choose *which* arrangement and *when*. These are the arrangements.

// The arrangements themselves are safe; `ffi` needs `unsafe` to export C
// symbols and to hand the preview a buffer to read, and says so at each use.
pub mod ffi;

/// A rectangle, in whatever coordinates the caller is using.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    #[must_use]
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// Shrunk by `amount` on every side.
    #[must_use]
    pub fn inset(self, amount: f64) -> Self {
        Self {
            x: self.x + amount,
            y: self.y + amount,
            w: (self.w - amount * 2.0).max(1.0),
            h: (self.h - amount * 2.0).max(1.0),
        }
    }

    /// This rectangle scaled to fit inside `box`, keeping its aspect ratio and
    /// never growing.
    ///
    /// Never growing matters: a small window blown up to fill an overview cell
    /// reads as a different window.
    #[must_use]
    pub fn fitted(self, into: Self) -> Self {
        if self.w <= 0.0 || self.h <= 0.0 {
            return into;
        }
        let scale = (into.w / self.w).min(into.h / self.h).min(1.0);
        let (w, h) = (self.w * scale, self.h * scale);
        Self {
            x: into.x + (into.w - w) / 2.0,
            y: into.y + (into.h - h) / 2.0,
            w,
            h,
        }
    }
}

/// How the standard arrangements are tuned.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Space between windows and around the edge.
    pub gap: f64,
    /// Share of the width the master window takes, for master-and-stack.
    pub ratio: f64,
    /// Share of the width one column takes, for the scrolling strip.
    pub column: f64,
    /// Space around each thumbnail in the grid.
    pub padding: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gap: 12.0,
            ratio: 0.6,
            column: 0.44,
            padding: 24.0,
        }
    }
}

/// One large window, the rest stacked beside it.
///
/// The arrangement worth having first, and the one most tiling users reach for
/// before anything else.
#[must_use]
pub fn master_stack(count: usize, area: Rect, settings: Settings) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let area = area.inset(settings.gap);
    if count == 1 {
        return vec![area];
    }

    let master = (area.w * settings.ratio - settings.gap / 2.0).max(1.0);
    let mut slots = vec![Rect::new(area.x, area.y, master, area.h)];

    #[expect(
        clippy::cast_precision_loss,
        reason = "a window count large enough to lose precision is not reachable"
    )]
    let stacked = (count - 1) as f64;
    let height = ((area.h - settings.gap * (stacked - 1.0)) / stacked).max(1.0);

    for index in 1..count {
        #[expect(clippy::cast_precision_loss, reason = "as above")]
        let row = (index - 1) as f64;
        slots.push(Rect::new(
            area.x + master + settings.gap,
            area.y + row * (height + settings.gap),
            (area.w - master - settings.gap).max(1.0),
            height,
        ));
    }
    slots
}

/// An endless horizontal strip, with the area as a viewport.
///
/// Windows keep their width and run off both edges; `offset` moves the view,
/// never the strip. Nothing is squeezed to fit, which is what makes this work
/// on a screen too small for anything else.
#[must_use]
pub fn scrolling(count: usize, area: Rect, settings: Settings, offset: f64) -> Vec<Rect> {
    let inner = area.inset(settings.gap);
    let width = (area.w * settings.column).max(1.0);

    (0..count)
        .map(|index| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a window count large enough to lose precision is not reachable"
            )]
            let column = index as f64;
            Rect::new(
                inner.x + column * (width + settings.gap) - offset,
                inner.y,
                width,
                inner.h,
            )
        })
        .collect()
}

/// How far the viewport must move to bring a column fully into view.
///
/// Returns the new offset. A column already on screen does not move the view:
/// scrolling something into sight that is already in sight is the kind of
/// motion that makes a layout feel twitchy.
#[must_use]
pub fn scroll_to(index: usize, count: usize, area: Rect, settings: Settings, offset: f64) -> f64 {
    if count == 0 {
        return 0.0;
    }
    let index = index.min(count - 1);
    let width = (area.w * settings.column).max(1.0);
    #[expect(
        clippy::cast_precision_loss,
        reason = "a window count large enough to lose precision is not reachable"
    )]
    let left = index as f64 * (width + settings.gap);
    let viewport = area.w - settings.gap * 2.0;

    if left < offset {
        left
    } else if left + width > offset + viewport {
        left + width - viewport
    } else {
        offset
    }
}

/// A grid of thumbnails, as close to square as the count allows.
///
/// `sizes` are the windows' own rectangles, so each keeps its aspect ratio
/// inside its cell — the arrangement overview uses.
#[must_use]
pub fn grid(sizes: &[Rect], area: Rect, settings: Settings) -> Vec<Rect> {
    let count = sizes.len();
    if count == 0 {
        return Vec::new();
    }

    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a window count large enough to lose precision is not reachable"
    )]
    let columns = ((count as f64).sqrt().ceil() as usize).max(1);
    let rows = count.div_ceil(columns);

    #[expect(clippy::cast_precision_loss, reason = "grid dimensions are small")]
    let (columns_f, rows_f) = (columns as f64, rows.max(1) as f64);
    let (cell_w, cell_h) = (area.w / columns_f, area.h / rows_f);

    sizes
        .iter()
        .enumerate()
        .map(|(index, size)| {
            #[expect(clippy::cast_precision_loss, reason = "grid dimensions are small")]
            let (column, row) = ((index % columns) as f64, (index / columns) as f64);
            let cell = Rect::new(
                area.x + column * cell_w + settings.padding,
                area.y + row * cell_h + settings.padding,
                (cell_w - settings.padding * 2.0).max(1.0),
                (cell_h - settings.padding * 2.0).max(1.0),
            );
            size.fitted(cell)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 34.0,
        w: 1600.0,
        h: 866.0,
    };

    #[test]
    fn one_window_fills_the_area_minus_the_gap() {
        let slots = master_stack(1, AREA, Settings::default());
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0], AREA.inset(12.0));
    }

    #[test]
    fn master_and_stack_share_the_width_without_overlapping() {
        let settings = Settings::default();
        let slots = master_stack(3, AREA, settings);
        assert_eq!(slots.len(), 3);

        let master = slots[0];
        assert!(
            master.x + master.w <= slots[1].x,
            "the stack must start after the master ends"
        );
        // And the stack does not overlap itself.
        assert!(slots[1].y + slots[1].h <= slots[2].y);
    }

    #[test]
    fn every_slot_stays_inside_the_area() {
        for count in 1..8 {
            for slot in master_stack(count, AREA, Settings::default()) {
                assert!(
                    slot.x >= AREA.x,
                    "count {count}: {slot:?} starts left of the area"
                );
                assert!(
                    slot.y >= AREA.y,
                    "count {count}: {slot:?} starts above the area"
                );
                assert!(
                    slot.x + slot.w <= AREA.x + AREA.w + 0.001,
                    "count {count}: {slot:?} runs past the right edge"
                );
                assert!(
                    slot.y + slot.h <= AREA.y + AREA.h + 0.001,
                    "count {count}: {slot:?} runs past the bottom"
                );
            }
        }
    }

    #[test]
    fn the_strip_keeps_its_column_width_however_many_there_are() {
        let settings = Settings::default();
        let two = scrolling(2, AREA, settings, 0.0);
        let ten = scrolling(10, AREA, settings, 0.0);
        assert!(
            (two[0].w - ten[0].w).abs() < 1e-9,
            "columns must not shrink to fit"
        );
        // And it runs off the edge rather than being squeezed.
        assert!(ten[9].x + ten[9].w > AREA.x + AREA.w);
    }

    #[test]
    fn scrolling_moves_the_view_and_not_the_strip() {
        let settings = Settings::default();
        let at_rest = scrolling(5, AREA, settings, 0.0);
        let scrolled = scrolling(5, AREA, settings, 300.0);
        for (a, b) in at_rest.iter().zip(&scrolled) {
            assert!(
                (a.x - b.x - 300.0).abs() < 1e-9,
                "every column moves by the same amount"
            );
            assert!((a.w - b.w).abs() < 1e-9, "and none of them changes size");
        }
    }

    #[test]
    fn a_column_already_in_view_does_not_move_the_viewport() {
        let settings = Settings::default();
        assert!((scroll_to(0, 5, AREA, settings, 0.0) - 0.0).abs() < 1e-9);
        // The far one does.
        assert!(scroll_to(4, 5, AREA, settings, 0.0) > 0.0);
    }

    #[test]
    fn the_grid_keeps_each_window_its_own_shape() {
        // A wide window and a tall one in the same grid must not come out the
        // same shape as each other.
        let wide = Rect::new(0.0, 0.0, 1200.0, 400.0);
        let tall = Rect::new(0.0, 0.0, 400.0, 1000.0);
        let slots = grid(&[wide, tall], AREA, Settings::default());
        let ratio = |r: Rect| r.w / r.h;
        assert!((ratio(slots[0]) - ratio(wide)).abs() < 1e-6);
        assert!((ratio(slots[1]) - ratio(tall)).abs() < 1e-6);
    }

    #[test]
    fn nothing_divides_by_zero_on_an_empty_desktop() {
        assert!(master_stack(0, AREA, Settings::default()).is_empty());
        assert!(scrolling(0, AREA, Settings::default(), 0.0).is_empty());
        assert!(grid(&[], AREA, Settings::default()).is_empty());
        assert!((scroll_to(0, 0, AREA, Settings::default(), 0.0) - 0.0).abs() < 1e-9);
    }
}
