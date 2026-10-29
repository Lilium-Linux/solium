//! At most one pass a refresh for each monitor on the hardware (#392).
//!
//! A screen that queued a frame is `pending` until its flip, and the flip's
//! vblank asks for the next pass: one pass a refresh. A pass that found
//! nothing to queue on a screen has no flip coming, and nothing used to hold
//! that screen back from the next pass, so every event the loop woke for --
//! each report of a 1000 Hz mouse, each commit of a client -- drew it again.
//! Measured on a 260 Hz and a 75 Hz monitor, that was 1,000 to 2,000 passes a
//! second against 335 vblanks, each paying for `render::prepare` in full, and
//! the main thread at 87% of a core.
//!
//! So a screen that drew nothing rests until its next vblank, the way a
//! screen with a frame in flight waits for its flip, and the end of its rest
//! asks for the pass the flip would have asked for. What changed meanwhile is
//! drawn then, on the vblank's phase, so it reaches the screen at the same
//! vblank it would have reached it at had it been drawn at once: a frame drawn
//! between two vblanks flips at the second either way.

use std::time::Duration;

/// One screen's rest. `tty::Screen` holds one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cadence {
    /// CLOCK_MONOTONIC at its last flip: the phase its vblanks keep.
    flipped: Option<Duration>,
    /// No pass draws it before this, on CLOCK_MONOTONIC.
    until: Option<Duration>,
}

impl Cadence {
    /// Its frame flipped at `at`: the phase of every vblank after it, and no
    /// rest, since the flip asks for the next pass itself.
    pub(crate) fn flipped(&mut self, at: Duration) {
        self.flipped = Some(at);
        self.until = None;
    }

    /// A pass drew it at `now` and queued nothing: it rests until its next
    /// vblank, which is returned, for the timer that ends the rest.
    /// `tests::a_screen_that_drew_nothing_rests_until_its_next_vblank`.
    pub(crate) fn drew_nothing(&mut self, now: Duration, interval: Duration) -> Duration {
        let until = next_vblank(self.flipped, now, interval);
        self.until = Some(until);
        until
    }

    /// Whether a pass at `now` must leave it alone.
    pub(crate) fn resting(&self, now: Duration) -> bool {
        self.until.is_some_and(|until| now < until)
    }

    /// The timer for its rest fired: the rest is over, whatever the clock
    /// says. A timer that fired a little early must not leave the screen
    /// resting with no timer left to end it.
    /// `tests::the_timer_ends_the_rest_even_a_little_early`.
    pub(crate) fn woke(&mut self) {
        self.until = None;
    }
}

/// The first vblank after `now` of a screen that last flipped at `flipped`
/// and refreshes every `interval`; a refresh from now for one that has not
/// flipped yet.
/// `tests::the_next_vblank_keeps_the_phase_of_the_last_flip`.
pub(crate) fn next_vblank(
    flipped: Option<Duration>,
    now: Duration,
    interval: Duration,
) -> Duration {
    let step = interval.as_nanos();
    match flipped {
        Some(flipped) if step > 0 && flipped <= now => {
            let elapsed = (now - flipped).as_nanos();
            let next = (elapsed / step + 1) * step;
            flipped + Duration::from_nanos(u64::try_from(next).unwrap_or(u64::MAX))
        }
        _ => now + interval,
    }
}

#[cfg(test)]
mod tests {
    use super::{Cadence, next_vblank};
    use std::time::Duration;

    fn us(value: u64) -> Duration {
        Duration::from_micros(value)
    }

    /// **The next vblank keeps the phase of the last flip**: a rest ends on
    /// the screen's own vblank, not a refresh after whenever the pass ran.
    #[test]
    fn the_next_vblank_keeps_the_phase_of_the_last_flip() {
        let interval = us(4_000);
        assert_eq!(
            next_vblank(Some(us(10_000)), us(11_000), interval),
            us(14_000)
        );
        assert_eq!(
            next_vblank(Some(us(10_000)), us(14_000), interval),
            us(18_000)
        );
        assert_eq!(
            next_vblank(Some(us(10_000)), us(53_500), interval),
            us(54_000)
        );
        // No flip to keep the phase of, or one from a clock that is ahead.
        assert_eq!(next_vblank(None, us(11_000), interval), us(15_000));
        assert_eq!(
            next_vblank(Some(us(20_000)), us(11_000), interval),
            us(15_000)
        );
    }

    /// **A screen that drew nothing rests until its next vblank**, and a
    /// flip ends a rest: the flip asks for the next pass itself.
    #[test]
    fn a_screen_that_drew_nothing_rests_until_its_next_vblank() {
        let interval = us(4_000);
        let mut cadence = Cadence::default();
        assert!(!cadence.resting(us(0)), "a new screen rests");
        cadence.flipped(us(10_000));
        assert_eq!(cadence.drew_nothing(us(11_000), interval), us(14_000));
        assert!(cadence.resting(us(11_001)));
        assert!(cadence.resting(us(13_999)));
        assert!(!cadence.resting(us(14_000)), "the vblank ends the rest");
        cadence.drew_nothing(us(14_100), interval);
        cadence.flipped(us(15_000));
        assert!(!cadence.resting(us(15_001)), "a flip leaves it resting");
    }

    #[test]
    fn the_timer_ends_the_rest_even_a_little_early() {
        let mut cadence = Cadence::default();
        cadence.drew_nothing(us(1_000), us(4_000));
        assert!(cadence.resting(us(4_900)));
        cadence.woke();
        assert!(!cadence.resting(us(4_900)));
    }
}
