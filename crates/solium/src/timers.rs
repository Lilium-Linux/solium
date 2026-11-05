//! `sol.after` and `sol.every` (#472): timers on the compositor's clock.
//!
//! Lua keeps each timer's function in `sol._timers`, by id; this keeps when
//! each is due. [`settle`] checks once each time round the event loop, which
//! wakes at least every 16 ms, and tells the configuration every timer due by
//! then in one dispatch, in the order they fall due, and by creation among
//! timers due at the same moment. One that falls due again before the next
//! check, an `every` shorter than a loop or a timer set from inside a timer,
//! waits for that check, so a timer that keeps setting itself cannot hold the
//! loop.
//!
//! The clock is `Solium::clock`, the one compositor animations and QML run
//! on, so a test that moves it moves the timers with everything else.

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use crate::state::Solium;

/// When every timer is next due.
#[derive(Debug, Default)]
pub(crate) struct Timers {
    /// By when it is due, then by id, which counts up as timers are made:
    /// the order they are told in. Each holds its period, for `every`.
    queue: BTreeMap<(Duration, u64), Option<Duration>>,
    /// When each is due, to find it by id.
    due_at: HashMap<u64, Duration>,
}

impl Timers {
    /// Set timer `id` to fall due `after` from `now`, and every `period`
    /// after that if it has one.
    pub(crate) fn add(
        &mut self,
        id: u64,
        now: Duration,
        after: Duration,
        period: Option<Duration>,
    ) {
        self.cancel(id);
        let due = now.saturating_add(after);
        self.queue.insert((due, id), period);
        self.due_at.insert(id, due);
    }

    pub(crate) fn cancel(&mut self, id: u64) {
        if let Some(due) = self.due_at.remove(&id) {
            self.queue.remove(&(due, id));
        }
    }

    /// Forget every timer, for a reload.
    pub(crate) fn clear(&mut self) {
        self.queue.clear();
        self.due_at.clear();
    }

    /// The timers due by `now`, in order, each with whether it was its last
    /// time (`sol.after`). An `every` is set again a period after it was
    /// due, or a period from `now` when that has passed too, so one the loop
    /// fell behind on is told once rather than once for each period missed.
    pub(crate) fn due(&mut self, now: Duration) -> Vec<(u64, bool)> {
        let mut due = Vec::new();
        while let Some(entry) = self.queue.first_entry() {
            let (at, id) = *entry.key();
            if at > now {
                break;
            }
            let period = entry.remove();
            self.due_at.remove(&id);
            due.push((id, period.is_none()));
            if let Some(period) = period {
                let mut next = at.saturating_add(period);
                if next <= now {
                    next = now.saturating_add(period);
                }
                self.queue.insert((next, id), Some(period));
                self.due_at.insert(id, next);
            }
        }
        due
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// How many timers are set, for a test.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.queue.len()
    }
}

/// Once a loop: tell the configuration the timers due by now.
pub(crate) fn settle(state: &mut Solium) {
    if state.timers.is_empty() {
        return;
    }
    let due = state.timers.due(state.clock.now());
    if due.is_empty() {
        return;
    }
    let snapshot = state.snapshot();
    let Some(mut scripts) = state.scripts.take() else {
        return;
    };
    let outcome = scripts.timers_due(&due, snapshot);
    state.scripts = Some(scripts);
    state.apply(outcome);
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// **Timers fall due in order of when, then of creation.**
    #[test]
    fn timers_fall_due_by_when_then_by_creation() {
        let mut timers = Timers::default();
        timers.add(3, ms(0), ms(50), None);
        timers.add(1, ms(0), ms(100), None);
        timers.add(2, ms(0), ms(50), None);
        assert_eq!(timers.due(ms(49)), []);
        assert_eq!(timers.due(ms(100)), [(2, true), (3, true), (1, true)]);
        assert!(timers.is_empty(), "a one-shot timer was kept");
    }

    /// **An `every` comes round each period**, and once only for periods the
    /// loop missed.
    #[test]
    fn an_every_comes_round_each_period_and_once_for_a_missed_run() {
        let mut timers = Timers::default();
        timers.add(1, ms(0), ms(10), Some(ms(10)));
        assert_eq!(timers.due(ms(10)), [(1, false)]);
        assert_eq!(timers.due(ms(19)), []);
        assert_eq!(timers.due(ms(20)), [(1, false)]);
        assert_eq!(
            timers.due(ms(75)),
            [(1, false)],
            "told once per missed period"
        );
        assert_eq!(timers.due(ms(84)), []);
        assert_eq!(timers.due(ms(85)), [(1, false)]);
    }

    /// **A cancelled timer never falls due**, and clearing forgets them all.
    #[test]
    fn a_cancelled_timer_never_falls_due_and_clear_forgets_all() {
        let mut timers = Timers::default();
        timers.add(1, ms(0), ms(10), None);
        timers.add(2, ms(0), ms(10), Some(ms(10)));
        timers.cancel(1);
        assert_eq!(timers.due(ms(10)), [(2, false)]);
        timers.cancel(2);
        assert_eq!(timers.due(ms(1000)), []);
        timers.add(3, ms(0), ms(10), None);
        timers.clear();
        assert_eq!(timers.due(ms(1000)), []);
        assert_eq!(timers.len(), 0);
    }
}
