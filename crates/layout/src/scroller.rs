//! Scrolling, as niri does it: columns, and a view anchored to one of them.
//!
//! Reimplemented from niri's `src/layout/scrolling.rs` (GPL-3.0-or-later,
//! compatible with this project) — the behaviour is the specification; the
//! code there is written against niri's own tile and animation types.
//!
//! Two ideas carry the whole design, and a strip of evenly-spaced windows has
//! neither of them.
//!
//! **The unit is a column, not a window.** A column holds a stack of windows
//! sharing its width, and its width is a share of the *view*, never of the
//! strip. Opening a tenth window cannot make the first nine thinner, because
//! nothing in here divides anything by how many windows exist. That is the
//! property that makes a scroller work the same on a laptop and a 49-inch
//! display.
//!
//! **The view offset is measured from the active column**, not from the left
//! end of the strip. Moving focus rebases it by the distance between the old
//! column and the new one, so the view and the focus can never disagree about
//! where they are — which is what going wrong feels like when a scroller feels
//! "fake". An absolute scroll position would drift away from focus the moment
//! a column ahead of it changed width.

use crate::{Rect, Settings};

/// A column of the strip.
#[derive(Clone, Debug)]
pub struct Column {
    /// Windows stacked in this column, top to bottom.
    pub windows: Vec<u64>,
    /// Which of them has focus.
    pub active: usize,
    /// Width as a share of the view.
    pub width: f64,
}

impl Column {
    fn new(id: u64, width: f64) -> Self {
        Self {
            windows: vec![id],
            active: 0,
            width,
        }
    }
}

/// The widths a column cycles through when the caller names none, as shares of
/// the view.
///
/// A fallback rather than the policy since #117. `lua/config.lua` has
/// advertised a `scrolling.widths` list since it was written, and this constant
/// is why that list did nothing: the configuration named three numbers and the
/// strip used these three instead. A caller supplies its own through
/// [`Scroller::with_widths`]; these are what a caller that says nothing gets,
/// and what a caller whose list turns out to hold nothing usable falls back to.
pub const PRESETS: [f64; 3] = [1.0 / 3.0, 0.5, 2.0 / 3.0];

/// A scrolling workspace.
#[derive(Clone, Debug)]
pub struct Scroller {
    columns: Vec<Column>,
    active: usize,
    /// Where the view sits relative to the active column's left edge.
    view_offset: f64,
    /// The widths this strip cycles through, as shares of the view.
    ///
    /// Held per strip rather than carried in [`crate::Settings`] because
    /// `Settings` is `Copy` and is rebuilt from a Lua table on every call —
    /// a list allocated per keystroke to describe something that cannot change
    /// between them. It is also per strip in truth: the widths are read once,
    /// where the strip is made.
    ///
    /// **Never empty.** Every read indexes it, and the two constructors below
    /// are the only things that fill it.
    widths: Vec<f64>,
    preset: usize,
}

impl Default for Scroller {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            active: 0,
            view_offset: 0.0,
            widths: PRESETS.to_vec(),
            // A third of the view, not a half. Half-width columns mean two on
            // screen and everything else off the edge, which for a terminal is
            // far wider than anyone reads at.
            preset: 0,
        }
    }
}

impl Scroller {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A strip cycling through widths of the caller's choosing.
    ///
    /// `start` is a zero-based index into `widths` — the width a new column
    /// opens at, and the point the cycle begins from. `lua/config.lua` spells
    /// it `default_width` and counts from one, the way Lua counts; the
    /// subtraction happens at that boundary, not here.
    ///
    /// **What it does with a list it cannot use.** Anything not a share of a
    /// view has already been dropped by the caller, which is where the log is;
    /// this sees the survivors. If there are none it falls back to [`PRESETS`],
    /// because every read of `widths` indexes it and a strip with no widths at
    /// all has no width to open a column at. An out-of-range `start` is clamped
    /// to the last width rather than refused, for the same reason: there is no
    /// answer to "start at the fourth of three" that is not a choice, and
    /// refusing would mean a session with no scrolling layout over one number.
    #[must_use]
    pub fn with_widths(widths: &[f64], start: usize) -> Self {
        let widths: Vec<f64> = if widths.is_empty() {
            PRESETS.to_vec()
        } else {
            widths.to_vec()
        };
        let preset = start.min(widths.len() - 1);
        Self {
            widths,
            preset,
            ..Self::default()
        }
    }

    /// The width a column takes now, as a share of the view.
    ///
    /// `preset` is kept in range by both constructors and by `cycle_width`'s
    /// modulo, so this cannot miss; the `unwrap_or` is what stands in for the
    /// index that would panic if one of those three ever stopped being true,
    /// and the crate denies panicking outright.
    fn width_now(&self) -> f64 {
        self.widths
            .get(self.preset)
            .or_else(|| self.widths.first())
            .copied()
            .unwrap_or(PRESETS[0])
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    #[must_use]
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    #[must_use]
    pub fn active_column(&self) -> usize {
        self.active
    }

    /// The focused window, if there is one.
    #[must_use]
    pub fn focused(&self) -> Option<u64> {
        let column = self.columns.get(self.active)?;
        column.windows.get(column.active).copied()
    }

    #[must_use]
    pub fn contains(&self, id: u64) -> bool {
        self.columns
            .iter()
            .any(|column| column.windows.contains(&id))
    }

    /// Open a window in a new column, to the right of the active one.
    pub fn insert(&mut self, id: u64, area: Rect, settings: Settings) {
        if self.contains(id) {
            return;
        }
        let width = self.width_now();
        let at = if self.columns.is_empty() {
            0
        } else {
            self.active + 1
        };
        self.columns.insert(at, Column::new(id, width));
        self.focus_column(at, area, settings);
    }

    /// Add a window to the active column instead of beside it.
    pub fn insert_into_active(&mut self, id: u64, area: Rect, settings: Settings) {
        if self.contains(id) {
            return;
        }
        let Some(column) = self.columns.get_mut(self.active) else {
            self.insert(id, area, settings);
            return;
        };
        column.windows.push(id);
        column.active = column.windows.len() - 1;
    }

    pub fn remove(&mut self, id: u64) {
        let Some(index) = self
            .columns
            .iter()
            .position(|column| column.windows.contains(&id))
        else {
            return;
        };
        let column = &mut self.columns[index];
        column.windows.retain(|other| *other != id);
        if column.windows.is_empty() {
            self.columns.remove(index);
            if self.active >= self.columns.len() {
                self.active = self.columns.len().saturating_sub(1);
            }
        } else {
            column.active = column.active.min(column.windows.len() - 1);
        }
    }

    /// Focus a particular window, bringing its column into view.
    ///
    /// This is what makes clicking a half-visible column work: the click says
    /// which window, and the view follows. Without it a column can hold focus
    /// while hanging off the edge of the screen, which is the state that makes
    /// a scroller feel like it is fighting you.
    pub fn focus_window(&mut self, id: u64, area: Rect, settings: Settings) {
        let Some(column) = self
            .columns
            .iter()
            .position(|column| column.windows.contains(&id))
        else {
            return;
        };
        if let Some(row) = self.columns[column].windows.iter().position(|w| *w == id) {
            self.columns[column].active = row;
        }
        self.focus_column(column, area, settings);
    }

    /// Move focus one column left or right, taking the view with it.
    pub fn focus_sideways(&mut self, by: isize, area: Rect, settings: Settings) {
        if self.columns.is_empty() {
            return;
        }
        let last = self.columns.len() - 1;
        let target = usize::try_from(
            isize::try_from(self.active)
                .unwrap_or(0)
                .saturating_add(by)
                .clamp(0, isize::try_from(last).unwrap_or(0)),
        )
        .unwrap_or(0);
        self.focus_column(target, area, settings);
    }

    /// Move focus within the active column.
    pub fn focus_vertically(&mut self, by: isize) {
        let Some(column) = self.columns.get_mut(self.active) else {
            return;
        };
        let last = column.windows.len().saturating_sub(1);
        column.active = usize::try_from(
            isize::try_from(column.active)
                .unwrap_or(0)
                .saturating_add(by)
                .clamp(0, isize::try_from(last).unwrap_or(0)),
        )
        .unwrap_or(0);
    }

    /// Swap the active column with its neighbour, carrying focus along.
    pub fn move_column(&mut self, by: isize, area: Rect, settings: Settings) {
        if self.columns.len() < 2 {
            return;
        }
        let last = self.columns.len() - 1;
        let Ok(from) = isize::try_from(self.active) else {
            return;
        };
        let to = usize::try_from(
            from.saturating_add(by)
                .clamp(0, isize::try_from(last).unwrap_or(0)),
        )
        .unwrap_or(0);
        if to == self.active {
            return;
        }
        self.columns.swap(self.active, to);
        self.focus_column(to, area, settings);
    }

    /// Pull the next column's focused window into this one.
    pub fn consume(&mut self) {
        let next = self.active + 1;
        let Some(taken) = self.columns.get_mut(next).and_then(|column| {
            let index = column.active.min(column.windows.len().saturating_sub(1));
            (!column.windows.is_empty()).then(|| column.windows.remove(index))
        }) else {
            return;
        };
        if self.columns[next].windows.is_empty() {
            self.columns.remove(next);
        }
        if let Some(column) = self.columns.get_mut(self.active) {
            column.windows.push(taken);
            column.active = column.windows.len() - 1;
        }
    }

    /// Push the focused window out into a column of its own.
    pub fn expel(&mut self, area: Rect, settings: Settings) {
        let Some(column) = self.columns.get_mut(self.active) else {
            return;
        };
        if column.windows.len() < 2 {
            return;
        }
        let index = column.active.min(column.windows.len() - 1);
        let taken = column.windows.remove(index);
        column.active = column.active.min(column.windows.len() - 1);
        let width = column.width;
        let at = self.active + 1;
        self.columns.insert(at, Column::new(taken, width));
        self.focus_column(at, area, settings);
    }

    /// Move a window into the column another window is in.
    ///
    /// What a drag means in a scroller: not a free position, but a change of
    /// which column something belongs to. Dropping onto a window in another
    /// column stacks it there.
    pub fn move_to_column_of(&mut self, id: u64, target: u64, area: Rect, settings: Settings) {
        if id == target {
            return;
        }
        let Some(to) = self
            .columns
            .iter()
            .position(|column| column.windows.contains(&target))
        else {
            return;
        };
        let Some(from) = self
            .columns
            .iter()
            .position(|column| column.windows.contains(&id))
        else {
            return;
        };
        if from == to {
            return;
        }

        self.columns[from].windows.retain(|other| *other != id);
        let emptied = self.columns[from].windows.is_empty();
        if emptied {
            self.columns.remove(from);
        } else {
            let column = &mut self.columns[from];
            column.active = column.active.min(column.windows.len() - 1);
        }

        // The target may have shifted left if the column it followed was
        // removed.
        let to = if emptied && to > from { to - 1 } else { to };
        let Some(column) = self.columns.get_mut(to) else {
            return;
        };
        let at = column
            .windows
            .iter()
            .position(|other| *other == target)
            .map_or(column.windows.len(), |index| index + 1);
        column.windows.insert(at, id);
        column.active = at;
        self.focus_column(to, area, settings);
    }

    /// Widen or narrow the column a window sits in.
    ///
    /// Every window in a column shares its width, so a drag on one window's
    /// edge can only mean the column. Clamped so a column can neither vanish
    /// nor grow past the view — past that it would be left-aligned anyway and
    /// dragging further would appear to do nothing.
    pub fn widen(&mut self, id: u64, by: f64, area: Rect, settings: Settings) {
        let Some(index) = self
            .columns
            .iter()
            .position(|column| column.windows.contains(&id))
        else {
            return;
        };
        let column = &mut self.columns[index];
        column.width = (column.width + by).clamp(0.1, 1.0);
        self.focus_column(index, area, settings);
    }

    /// Cycle the active column through the preset widths.
    pub fn cycle_width(&mut self, area: Rect, settings: Settings) {
        self.preset = (self.preset + 1) % self.widths.len();
        let width = self.width_now();
        if let Some(column) = self.columns.get_mut(self.active) {
            column.width = width;
        }
        let active = self.active;
        self.focus_column(active, area, settings);
    }

    /// Scroll the view by a distance, without moving focus.
    pub fn scroll_by(&mut self, delta: f64) {
        self.view_offset += delta;
    }

    /// Where every window goes.
    #[must_use]
    pub fn layout(&self, area: Rect, settings: Settings) -> Vec<(u64, Rect)> {
        let view = area.inset(settings.gap);
        let origin = self.view_position(view, settings);
        let mut out = Vec::new();

        for (index, column) in self.columns.iter().enumerate() {
            let x = view.x + self.column_x(index, view, settings) - origin;
            let width = (view.w * column.width).max(1.0);
            let count = column.windows.len().max(1);
            #[expect(clippy::cast_precision_loss, reason = "a column holds a handful")]
            let rows = count as f64;
            let height = ((view.h - settings.gap * (rows - 1.0)) / rows).max(1.0);

            for (row, id) in column.windows.iter().enumerate() {
                #[expect(clippy::cast_precision_loss, reason = "as above")]
                let row = row as f64;
                out.push((
                    *id,
                    Rect::new(x, view.y + row * (height + settings.gap), width, height),
                ));
            }
        }
        out
    }

    /// The left edge of the view, in strip coordinates.
    fn view_position(&self, view: Rect, settings: Settings) -> f64 {
        self.column_x(self.active, view, settings) + self.view_offset
    }

    /// Where a column starts along the strip.
    fn column_x(&self, index: usize, view: Rect, settings: Settings) -> f64 {
        self.columns
            .iter()
            .take(index)
            .map(|column| (view.w * column.width).max(1.0) + settings.gap)
            .sum()
    }

    /// Point focus at a column and bring it into view.
    ///
    /// The offset is rebased first, by the distance between the old column and
    /// the new one. Skipping that is what makes a scroller feel disconnected:
    /// the view would keep an offset measured from a column that is no longer
    /// the one it is describing.
    fn focus_column(&mut self, index: usize, area: Rect, settings: Settings) {
        if self.columns.is_empty() {
            return;
        }
        let index = index.min(self.columns.len() - 1);
        let view = area.inset(settings.gap);

        let old_x = self.column_x(self.active, view, settings);
        let new_x = self.column_x(index, view, settings);
        self.view_offset += old_x - new_x;
        self.active = index;

        let width = (view.w * self.columns[index].width).max(1.0);
        let current = new_x + self.view_offset;
        self.view_offset = fit(current, view.w, new_x, width, settings.gap);
    }
}

/// Where the view should sit so a column is visible, moving as little as
/// possible.
///
/// Three rules, in order, and the order is the point:
///
///   * A column wider than the view is left-aligned — there is no arrangement
///     that shows all of it, so showing the start beats showing the middle.
///   * A column already fully visible does not move the view at all. A
///     scroller that recentres on every focus change makes the whole screen
///     lurch when nothing needed to move.
///   * Otherwise the alignment that travels less wins.
fn fit(current: f64, view_width: f64, column_x: f64, column_width: f64, gap: f64) -> f64 {
    if view_width <= column_width {
        return 0.0;
    }
    let padding = ((view_width - column_width) / 2.0).clamp(0.0, gap);
    let left = column_x - padding;
    let right = column_x + column_width + padding;

    if current <= left && right <= current + view_width {
        return current - column_x;
    }

    let to_left = (current - left).abs();
    let to_right = ((current + view_width) - right).abs();
    if to_left <= to_right {
        left - column_x
    } else {
        right - view_width - column_x
    }
}

#[cfg(test)]
mod tests {
    use super::{PRESETS, Scroller};
    use crate::{Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }
    fn settings() -> Settings {
        Settings {
            gap: 0.0,
            ..Settings::default()
        }
    }
    fn rect_of(scroller: &Scroller, id: u64) -> Rect {
        scroller
            .layout(area(), settings())
            .into_iter()
            .find(|(other, _)| *other == id)
            .expect("window is in the strip")
            .1
    }
    fn with(count: u64) -> Scroller {
        let mut scroller = Scroller::new();
        for id in 1..=count {
            scroller.insert(id, area(), settings());
        }
        scroller
    }

    /// The defining property. Nothing here divides by how many windows exist,
    /// so a tenth window cannot make the first nine thinner.
    #[test]
    fn a_column_keeps_its_width_however_many_there_are() {
        let two = rect_of(&with(2), 1).w;
        let ten = rect_of(&with(10), 1).w;
        assert!((two - ten).abs() < f64::EPSILON, "{two} vs {ten}");
    }

    #[test]
    fn a_new_window_opens_beside_the_active_column_not_at_the_end() {
        let mut scroller = with(3);
        scroller.focus_sideways(-2, area(), settings());
        assert_eq!(scroller.active_column(), 0);
        scroller.insert(99, area(), settings());
        assert_eq!(
            scroller.active_column(),
            1,
            "it went right of the active one"
        );
        assert_eq!(scroller.focused(), Some(99));
    }

    /// The view is measured from the active column, so focusing a column that
    /// is already fully on screen must not move anything at all.
    #[test]
    fn focusing_a_visible_column_does_not_move_the_view() {
        let mut scroller = with(2); // two half-width columns fill the view
        let before = rect_of(&scroller, 1).x;
        scroller.focus_sideways(-1, area(), settings());
        assert!((rect_of(&scroller, 1).x - before).abs() < 1.0);
    }

    #[test]
    fn focusing_a_column_off_the_edge_brings_it_into_view() {
        let scroller = with(4);
        let last = rect_of(&scroller, 4);
        assert!(last.x >= 0.0 && last.x + last.w <= 1000.0 + 1.0, "{last:?}");
    }

    /// Focus and view can never disagree: whatever is focused is on screen.
    #[test]
    fn the_focused_column_is_always_visible() {
        let mut scroller = with(6);
        for step in [-1, -1, -1, 1, 1, -4, 5] {
            scroller.focus_sideways(step, area(), settings());
            let focused = scroller.focused().expect("something is focused");
            let rect = rect_of(&scroller, focused);
            assert!(
                rect.x >= -1.0 && rect.x + rect.w <= 1001.0,
                "focused column {focused} off screen at {rect:?}"
            );
        }
    }

    #[test]
    fn windows_in_a_column_share_its_width_and_split_its_height() {
        let mut scroller = with(1);
        scroller.insert_into_active(2, area(), settings());
        scroller.insert_into_active(3, area(), settings());
        let (one, three) = (rect_of(&scroller, 1), rect_of(&scroller, 3));
        assert!(
            (one.w - three.w).abs() < f64::EPSILON,
            "same column, same width"
        );
        assert!(
            (one.h - 200.0).abs() < 1.0,
            "three share the height: {one:?}"
        );
        assert!(three.y > one.y);
    }

    #[test]
    fn consuming_takes_the_next_column_into_this_one() {
        let mut scroller = with(2);
        scroller.focus_sideways(-1, area(), settings());
        scroller.consume();
        assert_eq!(scroller.columns().len(), 1);
        assert_eq!(scroller.columns()[0].windows.len(), 2);
    }

    #[test]
    fn expelling_puts_a_window_back_in_its_own_column() {
        let mut scroller = with(1);
        scroller.insert_into_active(2, area(), settings());
        assert_eq!(scroller.columns().len(), 1);
        scroller.expel(area(), settings());
        assert_eq!(scroller.columns().len(), 2);
        assert_eq!(scroller.focused(), Some(2));
    }

    #[test]
    fn cycling_width_walks_the_presets() {
        let mut scroller = with(1);
        let first = rect_of(&scroller, 1).w;
        scroller.cycle_width(area(), settings());
        let second = rect_of(&scroller, 1).w;
        assert!((first - second).abs() > 1.0, "width changed");
        // A full lap returns to the same preset, so it takes as many steps as
        // there are presets — not one fewer.
        for _ in 0..PRESETS.len() {
            scroller.cycle_width(area(), settings());
        }
        assert!(
            (rect_of(&scroller, 1).w - second).abs() < 1.0,
            "and comes back around"
        );
    }

    /// **The configured widths are the widths, and the configured index is
    /// where the cycle starts.**
    ///
    /// `scrolling.widths` and `scrolling.default_width` were documented in
    /// `lua/config.lua` and read by nothing: [`PRESETS`] decided both, so a
    /// configuration naming quarters got thirds and could not tell whether it
    /// had misunderstood the setting or been ignored (#117). Two numbers
    /// nothing else in this file uses, so a regression here cannot hide behind
    /// another test agreeing with it by coincidence.
    #[test]
    fn a_strip_cycles_through_the_widths_it_was_given() {
        // Deliberately unlike PRESETS in both membership and length: a lap of
        // two proves the cycle is over *this* list rather than over three of
        // anything.
        let mut scroller = Scroller::with_widths(&[0.25, 0.8], 1);
        scroller.insert(1, area(), settings());
        assert!(
            (rect_of(&scroller, 1).w - 800.0).abs() < 1.0,
            "a new column opens at `default_width`, which is the second of the \
             two given: {:?}",
            rect_of(&scroller, 1)
        );

        scroller.cycle_width(area(), settings());
        assert!(
            (rect_of(&scroller, 1).w - 250.0).abs() < 1.0,
            "and cycling wraps to the first of them: {:?}",
            rect_of(&scroller, 1)
        );

        scroller.cycle_width(area(), settings());
        assert!(
            (rect_of(&scroller, 1).w - 800.0).abs() < 1.0,
            "a lap of a two-entry list is two steps, not three"
        );
    }

    /// A list with nothing usable in it, and an index past the end of one.
    ///
    /// Both are reachable from a `user.lua`, and neither may cost the session
    /// its scrolling layout — `with_widths` is called while the configuration
    /// is being read, where an error would take every other setting with it.
    #[test]
    fn an_unusable_width_list_falls_back_rather_than_failing() {
        let mut scroller = Scroller::with_widths(&[], 0);
        scroller.insert(1, area(), settings());
        assert!(
            (rect_of(&scroller, 1).w - 1000.0 * PRESETS[0]).abs() < 1.0,
            "an empty list is the shipped presets, not a column of no width"
        );

        let mut scroller = Scroller::with_widths(&[0.4, 0.6], 9);
        scroller.insert(1, area(), settings());
        assert!(
            (rect_of(&scroller, 1).w - 600.0).abs() < 1.0,
            "an index past the end is the last width, not a missing one"
        );
    }

    #[test]
    fn removing_a_column_keeps_focus_somewhere_real() {
        let mut scroller = with(3);
        scroller.remove(3);
        assert!(scroller.focused().is_some());
        scroller.remove(2);
        scroller.remove(1);
        assert!(scroller.is_empty());
        assert!(scroller.layout(area(), settings()).is_empty());
    }

    /// Removing a window from a stacked column leaves the column standing.
    #[test]
    fn removing_one_of_a_stack_leaves_the_column() {
        let mut scroller = with(1);
        scroller.insert_into_active(2, area(), settings());
        scroller.remove(1);
        assert_eq!(scroller.columns().len(), 1);
        assert_eq!(scroller.columns()[0].windows, vec![2]);
    }

    #[test]
    fn moving_a_column_carries_focus_with_it() {
        let mut scroller = with(3);
        let focused = scroller.focused();
        scroller.move_column(-1, area(), settings());
        assert_eq!(scroller.focused(), focused, "the window went with the move");
        assert_eq!(scroller.active_column(), 1);
    }

    /// A column wider than the view is left-aligned, because no offset shows
    /// all of it and showing the start beats showing the middle.
    #[test]
    fn an_oversized_column_is_left_aligned() {
        let mut scroller = Scroller::new();
        scroller.insert(1, area(), settings());
        for _ in 0..6 {
            scroller.cycle_width(area(), settings());
        }
        let wide = Rect::new(0.0, 0.0, 300.0, 600.0);
        let placed = scroller
            .layout(wide, settings())
            .into_iter()
            .next()
            .expect("one window");
        assert!(placed.1.x.abs() < 1.0, "left-aligned: {:?}", placed.1);
    }
}

#[cfg(test)]
mod focus_and_width {
    use super::Scroller;
    use crate::{Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }
    fn settings() -> Settings {
        Settings {
            gap: 0.0,
            ..Settings::default()
        }
    }
    fn rect_of(scroller: &Scroller, id: u64) -> Rect {
        scroller
            .layout(area(), settings())
            .into_iter()
            .find(|(other, _)| *other == id)
            .expect("in the strip")
            .1
    }

    /// Clicking a column that is only half on screen must bring it fully into
    /// view. A column holding focus while hanging off the edge is the state
    /// that makes a scroller feel like it is fighting you.
    #[test]
    fn focusing_a_half_visible_column_brings_it_fully_on_screen() {
        let mut scroller = Scroller::new();
        for id in 1..=4 {
            scroller.insert(id, area(), settings());
        }
        scroller.focus_sideways(-3, area(), settings());
        let off = rect_of(&scroller, 4);
        assert!(
            off.x + off.w > 1000.0,
            "window 4 starts off the edge: {off:?}"
        );

        scroller.focus_window(4, area(), settings());
        let now = rect_of(&scroller, 4);
        assert!(
            now.x >= -1.0 && now.x + now.w <= 1001.0,
            "still not fully visible: {now:?}"
        );
    }

    #[test]
    fn widening_changes_the_column_and_every_window_in_it() {
        let mut scroller = Scroller::new();
        scroller.insert(1, area(), settings());
        scroller.insert_into_active(2, area(), settings());
        let before = rect_of(&scroller, 1).w;
        scroller.widen(1, 0.2, area(), settings());
        let (one, two) = (rect_of(&scroller, 1), rect_of(&scroller, 2));
        assert!(one.w > before, "the column grew");
        assert!((one.w - two.w).abs() < f64::EPSILON, "both windows with it");
    }

    #[test]
    fn a_column_can_neither_vanish_nor_outgrow_the_view() {
        let mut scroller = Scroller::new();
        scroller.insert(1, area(), settings());
        scroller.widen(1, -10.0, area(), settings());
        assert!(rect_of(&scroller, 1).w >= 99.0, "floor holds");
        scroller.widen(1, 10.0, area(), settings());
        assert!(rect_of(&scroller, 1).w <= 1001.0, "ceiling holds");
    }
}

#[cfg(test)]
mod moving {
    use super::Scroller;
    use crate::{Rect, Settings};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }
    fn settings() -> Settings {
        Settings {
            gap: 0.0,
            ..Settings::default()
        }
    }

    #[test]
    fn a_window_dropped_on_another_column_joins_it() {
        let mut scroller = Scroller::new();
        for id in 1..=3 {
            scroller.insert(id, area(), settings());
        }
        assert_eq!(scroller.columns().len(), 3);
        scroller.move_to_column_of(3, 1, area(), settings());
        assert_eq!(scroller.columns().len(), 2, "one column absorbed another");
        assert!(scroller.columns()[0].windows.contains(&3));
        assert_eq!(scroller.focused(), Some(3), "focus went with it");
    }

    #[test]
    fn moving_out_of_a_stack_leaves_the_column_standing() {
        let mut scroller = Scroller::new();
        scroller.insert(1, area(), settings());
        scroller.insert_into_active(2, area(), settings());
        scroller.insert(3, area(), settings());
        scroller.move_to_column_of(2, 3, area(), settings());
        assert_eq!(scroller.columns().len(), 2);
        assert_eq!(scroller.columns()[0].windows, vec![1]);
        assert!(scroller.columns()[1].windows.contains(&2));
    }

    #[test]
    fn a_window_cannot_be_moved_onto_itself() {
        let mut scroller = Scroller::new();
        scroller.insert(1, area(), settings());
        scroller.insert(2, area(), settings());
        scroller.move_to_column_of(1, 1, area(), settings());
        assert_eq!(scroller.columns().len(), 2);
    }
}
