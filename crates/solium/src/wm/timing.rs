//! How long the window manager took to answer, counted as it goes and said
//! in one line when a session ends or a window manager is replaced.
//!
//! The number that matters is on the machine it runs on, under what that
//! machine is doing: an answer is waited for one frame, so "how often did
//! it not come in one" is a property of a session, not of a benchmark. A
//! count in a few hundred bytes answers it from any session's log.

use std::time::Duration;

/// Buckets an octave: a time is known to within a fifth.
const STEPS: u32 = 4;
/// Up to 2^26 microseconds, a minute: anything longer is the last bucket.
const OCTAVES: u32 = 26;

/// Every answer's time, as a count in each of a row of buckets that double
/// every [`STEPS`].
#[derive(Clone, Debug)]
pub(crate) struct Timing {
    buckets: Vec<u64>,
    answered: u64,
    /// Those that came after the compositor had gone on without them.
    late: u64,
    slowest: Duration,
    /// How many batches each event was, and how long their answers took
    /// together, by the event's name.
    events: std::collections::BTreeMap<&'static str, (u64, Duration)>,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            buckets: vec![0; (OCTAVES * STEPS + 1) as usize],
            answered: 0,
            late: 0,
            slowest: Duration::ZERO,
            events: std::collections::BTreeMap::new(),
        }
    }
}

/// How many events the line names: the ones there were most of.
const NAMED: usize = 4;

/// The bucket a time of `micros` is counted in.
fn bucket(micros: u64) -> usize {
    if micros == 0 {
        return 0;
    }
    // Anything past the last octave is the last bucket's.
    let micros = micros.min(1 << OCTAVES);
    let octave = micros.ilog2();
    // How far into its octave, in [`STEPS`] steps.
    let step = if octave == 0 {
        0
    } else {
        let below = 1u64 << octave;
        ((micros - below) * u64::from(STEPS) / below).min(u64::from(STEPS) - 1)
    };
    let at = u64::from(octave) * u64::from(STEPS) + step + 1;
    usize::try_from(at.min(u64::from(OCTAVES * STEPS))).unwrap_or(usize::MAX)
}

/// The time, in microseconds, that everything counted in bucket `at` is
/// under.
fn ceiling(at: usize) -> u64 {
    let Some(at) = u32::try_from(at).ok().and_then(|at| at.checked_sub(1)) else {
        return 0;
    };
    let below = 1u64 << (at / STEPS);
    below + below * u64::from(at % STEPS + 1) / u64::from(STEPS)
}

impl Timing {
    /// An answer came `after` its batch, which was the event `event`, was
    /// sent: `late` when the compositor had stopped waiting for it.
    pub(crate) fn answered(&mut self, event: &'static str, after: Duration, late: bool) {
        let (count, total) = self.events.entry(event).or_default();
        *count += 1;
        *total += after;
        let micros = u64::try_from(after.as_micros()).unwrap_or(u64::MAX);
        if let Some(count) = self.buckets.get_mut(bucket(micros)) {
            *count += 1;
        }
        self.answered += 1;
        self.late += u64::from(late);
        self.slowest = self.slowest.max(after);
    }

    /// The time, in microseconds, that `share` of the answers came within:
    /// the ceiling of the bucket that share falls in.
    fn within(&self, share: f64) -> u64 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "a count of answers, as a share of them"
        )]
        let wanted = ((self.answered as f64) * share).ceil() as u64;
        let mut seen = 0;
        for (at, count) in self.buckets.iter().enumerate() {
            seen += count;
            if seen >= wanted {
                return ceiling(at);
            }
        }
        0
    }

    /// The line a session's log ends with, or none when nothing was
    /// answered.
    pub(crate) fn said(&self) -> Option<String> {
        if self.answered == 0 {
            return None;
        }
        // The events there were most of, and what an answer to each took
        // on average: where a session's round trips go.
        let mut events: Vec<(&str, u64, Duration)> = self
            .events
            .iter()
            .map(|(name, (count, total))| (*name, *count, *total))
            .collect();
        events.sort_by(|one, other| other.1.cmp(&one.1).then(one.0.cmp(other.0)));
        let most: Vec<String> = events
            .iter()
            .take(NAMED)
            .map(|(name, count, total)| {
                let each = total.as_micros() / u128::from((*count).max(1));
                format!("{name} {count} ({each} us each)")
            })
            .collect();
        Some(format!(
            "{} batches answered: half within {} us, 99 in 100 within {} us, the slowest \
             {:.1} ms; {} came after the compositor had gone on; most were {}",
            self.answered,
            self.within(0.5),
            self.within(0.99),
            self.slowest.as_secs_f64() * 1000.0,
            self.late,
            most.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Timing, bucket, ceiling};

    /// A time is counted in a bucket whose ceiling is at least it, and no
    /// more than a quarter of an octave above it.
    #[test]
    fn a_time_is_counted_within_a_fifth_of_what_it_was() {
        for micros in [
            1u64, 2, 3, 7, 8, 19, 20, 21, 100, 999, 4000, 123_456, 9_000_000,
        ] {
            let top = ceiling(bucket(micros));
            assert!(top >= micros, "{micros} us is over its bucket's {top}");
            assert!(
                top <= micros + micros / 4 + 1,
                "{micros} us is counted as up to {top}"
            );
        }
        assert_eq!(bucket(0), 0);
        assert_eq!(bucket(u64::MAX), 26 * 4, "the last bucket takes the rest");
    }

    /// The line says how many, the middle, the tail, the worst, the late
    /// ones and which events there were most of, and nothing is said of a
    /// window manager that answered nothing.
    #[test]
    fn the_line_says_the_middle_the_tail_and_the_late() {
        let mut timing = Timing::default();
        assert_eq!(timing.said(), None);
        for _ in 0..98 {
            timing.answered("timers", Duration::from_micros(20), false);
        }
        timing.answered("opened", Duration::from_micros(300), false);
        timing.answered("key", Duration::from_millis(6), true);
        let said = timing.said().expect("a line");
        assert_eq!(
            said,
            "100 batches answered: half within 24 us, 99 in 100 within 320 us, the slowest \
             6.0 ms; 1 came after the compositor had gone on; most were timers 98 (20 us \
             each), key 1 (6000 us each), opened 1 (300 us each)",
            "each to the top of its bucket: 20 us is under 24, 300 under 320"
        );
    }
}
