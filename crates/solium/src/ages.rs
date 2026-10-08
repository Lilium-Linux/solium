//! The age a window buffer has for the damage tracker, which counts the frames
//! it recorded, and not the frames the window showed.
//!
//! A buffer's age is how many frames ago its contents were shown: the window
//! system counts swaps, the damage tracker counts every frame it recorded, and
//! a frame can be recorded and never swapped. The nested backend draws a
//! captured frame and reads it back without presenting it (`winit.rs`), and a
//! submit can fail. The tracker then holds one frame more than the window
//! has shown. Handed the window's age, it adds up one frame too few of its
//! old damage, and what changed in the frame left out stays stale in that
//! buffer. A moving window leaves the strip it uncovered in that frame, a line
//! where its edge was: #235 for a genie, #179 for the overview.
//! `tests::a_frame_drawn_and_not_shown_leaves_no_stale_strip`.

use std::collections::VecDeque;

/// The most frames kept: an age older than this many recorded frames is drawn
/// in full, which is what the tracker does with one past its own history.
const KEPT: usize = 16;

/// Every frame the damage tracker recorded, newest first, and whether each was
/// shown.
#[derive(Debug, Default)]
pub(crate) struct Ages {
    shown: VecDeque<bool>,
}

impl Ages {
    /// The age to hand the damage tracker for a buffer the window says is
    /// `buffer_age` frames old: how many recorded frames ago the frame shown
    /// `buffer_age` swaps ago was, counting the ones not shown in between.
    /// Zero, a full redraw, when that frame is not among them.
    /// `tests::a_frame_not_shown_makes_the_tracker_age_one_older`.
    ///
    /// Never too young, whatever the buffer holds. One drawn and not shown is
    /// kept as the back buffer and holds a newer frame than its age says, and
    /// what changed since the older frame covers what changed since the newer:
    /// more damage than it needs, and never less.
    pub(crate) fn for_tracker(&self, buffer_age: usize) -> usize {
        if buffer_age == 0 {
            return 0;
        }
        self.shown
            .iter()
            .enumerate()
            .filter(|(_, shown)| **shown)
            .nth(buffer_age - 1)
            .map_or(0, |(index, _)| index + 1)
    }

    /// The tracker recorded a frame, which the window `shown` or did not.
    /// Called only when it did record one: it records none when nothing was
    /// damaged (smithay `damage/mod.rs`, the early return).
    pub(crate) fn recorded(&mut self, shown: bool) {
        self.shown.push_front(shown);
        self.shown.truncate(KEPT);
    }

    /// The tracker forgot what it recorded: a failed render resets it.
    pub(crate) fn forget(&mut self) {
        self.shown.clear();
    }
}

#[cfg(test)]
mod tests {
    use smithay::{
        backend::renderer::{
            damage::OutputDamageTracker,
            element::{Id, Kind, solid::SolidColorRenderElement},
            utils::CommitCounter,
        },
        utils::{Physical, Rectangle, Transform},
    };

    use super::Ages;

    const SIZE: i32 = 64;

    /// A window's buffers, as Mesa's Wayland platform keeps them: the back
    /// buffer is kept until a swap, so a frame drawn and not shown is drawn
    /// over by the next one; each swap makes every shown buffer a frame older.
    /// The host hands them back in turn, which is what three-deep swapping
    /// gives (the age 3 the nested log shows on almost every frame).
    struct Swapchain {
        ages: Vec<usize>,
        pixels: Vec<Vec<u8>>,
        next: usize,
        back: Option<usize>,
    }

    impl Swapchain {
        fn new(count: usize) -> Self {
            Self {
                ages: vec![0; count],
                // Nothing a frame would draw: a pixel never painted is stale.
                pixels: vec![vec![u8::MAX; (SIZE * SIZE) as usize]; count],
                next: 0,
                back: None,
            }
        }

        fn back(&mut self) -> usize {
            let count = self.ages.len();
            let next = &mut self.next;
            *self.back.get_or_insert_with(|| {
                let back = *next;
                *next = (*next + 1) % count;
                back
            })
        }

        fn swap(&mut self) {
            for age in &mut self.ages {
                if *age > 0 {
                    *age += 1;
                }
            }
            if let Some(back) = self.back.take() {
                self.ages[back] = 1;
            }
        }
    }

    /// A window as a genie draws it: its top edge coming down a few pixels a
    /// frame and its sides drawing in, over a still wallpaper.
    fn window_at(frame: i32) -> Rectangle<i32, Physical> {
        let top = 4 + 3 * frame;
        Rectangle::new((4 + frame, top).into(), (56 - 2 * frame, 60 - top).into())
    }

    /// What a frame shows: 1 inside the window, 0 the wallpaper.
    fn picture(window: Rectangle<i32, Physical>) -> Vec<u8> {
        let mut pixels = vec![0; (SIZE * SIZE) as usize];
        for y in 0..SIZE {
            for x in 0..SIZE {
                if window.contains((x, y)) {
                    pixels[(y * SIZE + x) as usize] = 1;
                }
            }
        }
        pixels
    }

    /// Run sixteen frames through a real damage tracker, painting each one's
    /// damage into the back buffer and swapping it unless `captured` says the
    /// frame is drawn and not shown. Each frame's tracker age comes from
    /// `age_for`, given the bookkeeping and the window's age. The most pixels
    /// any frame left different from what it shows.
    fn stale(age_for: impl Fn(&Ages, usize) -> usize, captured: impl Fn(i32) -> bool) -> usize {
        let mut tracker = OutputDamageTracker::new((SIZE, SIZE), 1.0, Transform::Normal);
        let (window_id, wallpaper_id) = (Id::new(), Id::new());
        let mut commit = CommitCounter::default();
        let mut chain = Swapchain::new(3);
        let mut ages = Ages::default();
        let mut worst = 0;
        for frame in 0..16 {
            commit.increment();
            let window = window_at(frame);
            let elements = [
                SolidColorRenderElement::new(
                    window_id.clone(),
                    window,
                    commit,
                    [1.0, 1.0, 1.0, 1.0],
                    Kind::Unspecified,
                ),
                SolidColorRenderElement::new(
                    wallpaper_id.clone(),
                    Rectangle::from_size((SIZE, SIZE).into()),
                    CommitCounter::default(),
                    [0.0, 0.0, 0.0, 1.0],
                    Kind::Unspecified,
                ),
            ];
            let back = chain.back();
            let age = age_for(&ages, chain.ages[back]);
            let truth = picture(window);
            let (damage, _) = tracker.damage_output(age, &elements).expect("a mode");
            if let Some(damage) = damage {
                for rect in damage {
                    for y in rect.loc.y..rect.loc.y + rect.size.h {
                        for x in rect.loc.x..rect.loc.x + rect.size.w {
                            let at = (y * SIZE + x) as usize;
                            chain.pixels[back][at] = truth[at];
                        }
                    }
                }
                ages.recorded(!captured(frame));
            }
            let wrong = chain.pixels[back]
                .iter()
                .zip(&truth)
                .filter(|(drawn, shown)| drawn != shown)
                .count();
            worst = worst.max(wrong);
            if !captured(frame) {
                chain.swap();
            }
        }
        worst
    }

    /// **A frame drawn and not shown leaves no stale strip** (#235, #179): a
    /// capture every fourth frame of a moving window, and every buffer still
    /// holds exactly the frame it shows.
    ///
    /// The window's own age, handed straight to the tracker, is the control:
    /// it leaves the strip, so this model can see the defect at all.
    #[test]
    fn a_frame_drawn_and_not_shown_leaves_no_stale_strip() {
        let every_fourth = |frame: i32| frame % 4 == 2;
        assert!(
            stale(|_, window| window, every_fourth) > 0,
            "the control: the window's own age leaves a strip"
        );
        assert_eq!(
            stale(|ages, window| ages.for_tracker(window), every_fourth),
            0,
            "a stale strip where the window's edge was"
        );
    }

    /// With every frame shown the two ages are the same age.
    #[test]
    fn with_every_frame_shown_the_window_age_is_the_tracker_age() {
        let mut ages = Ages::default();
        for _ in 0..4 {
            ages.recorded(true);
        }
        assert_eq!(
            (1..=4).map(|age| ages.for_tracker(age)).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(stale(|ages, window| ages.for_tracker(window), |_| false), 0);
    }

    /// A frame not shown is counted as well: the buffer three shows old is
    /// four recorded frames old. Unknown stays unknown, and so does an age
    /// older than anything recorded.
    #[test]
    fn a_frame_not_shown_makes_the_tracker_age_one_older() {
        let mut ages = Ages::default();
        for shown in [true, true, true, false] {
            ages.recorded(shown);
        }
        assert_eq!(ages.for_tracker(3), 4);
        assert_eq!(ages.for_tracker(0), 0);
        assert_eq!(ages.for_tracker(4), 0);
        ages.forget();
        assert_eq!(ages.for_tracker(1), 0, "a reset tracker has nothing old");
    }
}
