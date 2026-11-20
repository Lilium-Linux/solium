//! Where the keyboard goes when the window that had it closes: the words of
//! `config.focus.after_close` (#128), and the geometry its `"neighbour"` is
//! chosen by.
//!
//! **Asked of geometry, because the compositor knows no layouts.** "The window
//! that takes its place" is a question about a tiling tree or a scrolling
//! strip, and neither lives on this side: they are the window manager's. What every layout
//! leaves behind is the same, though -- where each window is going -- so the
//! answer is read from that. Of the windows still on the monitor the closed
//! one was on, one going into the rectangle it stood in: in tiling the
//! sibling that grows into the space, in scrolling the column that slides
//! into it. Where none goes into any of it -- a strip that keeps its last
//! column where it was, a layout that waits for the application to go, a
//! floating desktop -- the one nearest to it. Between several, the window
//! focused most recently: two tiles that grow into one space take it by a
//! pixel more or less each, and which of them is a pixel wider says nothing
//! about which the user was working in.
//!
//! Each word is one way of answering; `config.focus.after_close` lists them in
//! order per mode, and the first that finds a window wins. See
//! `Solium::successor` in `state/keyboard.rs`, which reads them, and the tests
//! named on each word.

use smithay::utils::{Logical, Rectangle};

/// One word of `config.focus.after_close`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Successor {
    /// The window under the pointer, where windows are going: what hover
    /// focus would pick the moment the pointer moved.
    /// `with_after_close_pointer_the_window_under_the_pointer_takes_the_keyboard`.
    Pointer,
    /// The window that takes the closed one's place on its monitor, or the
    /// one nearest to where it stood: [`neighbour`].
    /// `in_tiling_the_window_that_takes_the_space_takes_the_keyboard`,
    /// `in_scrolling_the_column_that_takes_the_place_takes_the_keyboard`.
    Neighbour,
    /// The window focused most recently that is still open on the closed
    /// one's monitor.
    /// `in_floating_the_window_focused_before_it_on_that_monitor_takes_the_keyboard`.
    Recent,
    /// The topmost window on screen, on any monitor: what the compositor did
    /// for every window that went before #128, and still does with no
    /// configuration.
    Topmost,
}

impl Successor {
    /// What the compositor does when no configuration has said: the window
    /// under the pointer, else the topmost on screen. The rule `settle_focus`
    /// always had, so a session with no scripts -- and every test that runs
    /// none -- is unchanged.
    pub(crate) const UNCONFIGURED: [Self; 2] = [Self::Pointer, Self::Topmost];

    /// Every word, as `config.lua` spells it, for the log line that names
    /// one it does not know.
    pub(crate) const WORDS: [&'static str; 4] = ["pointer", "neighbour", "recent", "topmost"];

    /// A word of `config.focus.after_close`, or `None` for one it does not
    /// have.
    pub(crate) fn parse(word: &str) -> Option<Self> {
        match word {
            "pointer" => Some(Self::Pointer),
            "neighbour" => Some(Self::Neighbour),
            "recent" => Some(Self::Recent),
            "topmost" => Some(Self::Topmost),
            _ => None,
        }
    }
}

/// Whether two rectangles share any area: touching edges do not.
fn shares(one: Rectangle<i32, Logical>, other: Rectangle<i32, Logical>) -> bool {
    one.intersection(other)
        .is_some_and(|both| both.size.w > 0 && both.size.h > 0)
}

/// How far apart two rectangles are, edge to edge, squared: zero when they
/// touch or overlap.
fn apart(one: Rectangle<i32, Logical>, other: Rectangle<i32, Logical>) -> i64 {
    let gap = |start: i32, size: i32, other_start: i32, other_size: i32| -> i64 {
        let (end, other_end) = (
            i64::from(start) + i64::from(size),
            i64::from(other_start) + i64::from(other_size),
        );
        (i64::from(other_start) - end)
            .max(i64::from(start) - other_end)
            .max(0)
    };
    let dx = gap(one.loc.x, one.size.w, other.loc.x, other.size.w);
    let dy = gap(one.loc.y, one.size.h, other.loc.y, other.size.h);
    dx * dx + dy * dy
}

/// The candidate that takes the place of a window that stood at `stood`.
///
/// `candidates` are each window's key and where it is going, in order of
/// preference -- the caller lists them most recently focused first. The
/// first going into any of `stood` wins; with none going into it, the
/// nearest, edge to edge, and between equally near ones the first.
pub(crate) fn neighbour<K: Copy>(
    stood: Rectangle<i32, Logical>,
    candidates: &[(K, Rectangle<i32, Logical>)],
) -> Option<K> {
    candidates
        .iter()
        .find(|(_, rect)| shares(stood, *rect))
        .or_else(|| {
            // `min_by_key` answers the first of several equal ones.
            candidates
                .iter()
                .min_by_key(|(_, rect)| apart(stood, *rect))
        })
        .map(|(key, _)| *key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// **A window going into where the closed one stood takes its place**,
    /// whatever order it is listed in, over one beside it that goes into
    /// none of it.
    #[test]
    fn a_window_going_into_the_space_takes_the_place() {
        let stood = rect(0, 0, 500, 1000);
        let grew = rect(0, 500, 1000, 500);
        let beside = rect(512, 0, 488, 488);
        assert_eq!(neighbour(stood, &[(1, beside), (2, grew)]), Some(2));
        assert_eq!(neighbour(stood, &[(2, grew), (1, beside)]), Some(2));
    }

    /// **With nothing in the space, the nearest window takes it**, edge to
    /// edge rather than centre to centre: a tall window right beside it is
    /// nearer than a small one whose centre happens to be close.
    #[test]
    fn with_nothing_in_the_space_the_nearest_window_takes_it() {
        let stood = rect(500, 0, 500, 1000);
        let far = rect(1200, 400, 100, 100);
        let next_to = rect(0, 0, 488, 1000);
        assert_eq!(neighbour(stood, &[(1, far), (2, next_to)]), Some(2));
    }

    /// **Between two going into the space, the one listed first takes it**,
    /// which the caller makes the one focused most recently -- however much
    /// of the space each covers -- and so between two equally near.
    #[test]
    fn several_in_the_space_go_to_the_window_listed_first() {
        let stood = rect(0, 0, 500, 1000);
        let wider = rect(0, 0, 1000, 600);
        let narrower = rect(0, 612, 1000, 388);
        assert_eq!(neighbour(stood, &[(1, wider), (2, narrower)]), Some(1));
        assert_eq!(neighbour(stood, &[(2, narrower), (1, wider)]), Some(2));
        let left = rect(-600, 0, 588, 1000);
        let right = rect(512, 0, 588, 1000);
        assert_eq!(neighbour(stood, &[(3, right), (4, left)]), Some(3));
        assert_eq!(neighbour(stood, &[(4, left), (3, right)]), Some(4));
        assert_eq!(neighbour::<u8>(stood, &[]), None);
    }

    /// **Every word `config.lua` documents is one this reads**, and
    /// nothing else is.
    #[test]
    fn every_documented_word_parses_and_nothing_else_does() {
        for word in Successor::WORDS {
            assert!(Successor::parse(word).is_some(), "{word}");
        }
        assert_eq!(Successor::parse("neighbor"), None);
        assert_eq!(Successor::parse(""), None);
    }
}
