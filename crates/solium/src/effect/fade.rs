//! A rule's slot fading in and out as the rule comes and goes: what a rule's
//! `fade` asks for (amended 2026-10-15, the glass shade, which is a `front`
//! slot on a window without the keyboard that comes and goes with the focus).
//!
//! **The fade is the compositor's and draws nothing new.** The slot's result is
//! composited at the opacity this works out, as a decoration layer's picture is
//! at its `layerOpacity`: the chain is not run again for it, and a slot that
//! has faded out is wanted no more, so it costs nothing until its rule comes
//! back. Pure: one pass's answer from the last, the rule the slot resolved to
//! now, and the pass's instant. `tests`.
//!
//! **A slot that changes rule crossfades** (#389: a window eased between two
//! opacities as it gains and loses the keyboard, by two `replace` rules on
//! `focused`). When the new rule has a fade, the old rule's result is kept,
//! in the slot's `Leaving` twin, and both are drawn in the slot's place, mixed
//! in one draw as the crossfade moves (`effect::element::CrossfadeElement`):
//! each pixel the old result's and the new one's, weighted, and that drawn
//! over what is under the slot once. Not one over the other at the opposite
//! weights: the one on top would cover the one below as well as add to it,
//! so two pictures of a window at 0.95 and 0.80 would meet half way at 0.69
//! and not 0.875, a dip in the middle of every change of focus.
//!
//! **A `replace` comes from and goes to its part, not nothing.** Its result
//! takes its part's place, so fading it out over nothing would show what is
//! behind the window through it, and the part would pop back at the end (a
//! window going fullscreen, its see-through rule on `fullscreen = false`
//! stopping). For a `replace`, the opacity worked out here is the result's
//! share of a mix with the part's own pixels, drawn in the same one draw
//! (`render::want_resolved`, `effect::crossfade`).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::plan::Owner;
use super::rules::{Fade, RuleKey, Slot};

/// One value on its way between two others: `from` at `since` toward `to`,
/// on `fade`'s curve. A slot's opacity is one, and a crossfade's share is
/// another.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Leg {
    from: f32,
    to: f32,
    since: Duration,
    fade: Fade,
}

impl Leg {
    /// From 0 to 1 over `fade`, starting at `now`.
    fn rising(fade: Fade, now: Duration) -> Self {
        Self::rising_from(0.0, fade, now)
    }

    /// From `from` to 1 over the share of `fade` the way left takes.
    fn rising_from(from: f32, fade: Fade, now: Duration) -> Self {
        Self {
            from,
            to: 1.0,
            since: now,
            fade,
        }
    }

    /// The share of `fade.duration` this leg takes: the whole of it from 0 to
    /// 1, and as much of it as the way left when it turns round part of the
    /// way.
    fn span(&self) -> Duration {
        let share = f64::from((self.to - self.from).abs());
        Duration::try_from_secs_f64(self.fade.duration.as_secs_f64() * share)
            .unwrap_or(Duration::ZERO)
    }

    /// Where the value is at `now`, clamped to 0..1 (a curve that
    /// overshoots, `outBack` or a spring, is held at the ends).
    fn at(&self, now: Duration) -> f32 {
        let span = self.span();
        if span.is_zero() {
            return self.to;
        }
        let progress = now.saturating_sub(self.since).as_secs_f64() / span.as_secs_f64();
        let eased = self.fade.curve.at(progress.clamp(0.0, 1.0));
        let at = f64::from(self.from) + f64::from(self.to - self.from) * eased;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an opacity or a share, clamped to 0..1"
        )]
        let at = at.clamp(0.0, 1.0) as f32;
        at
    }

    /// Whether this leg has arrived by `now`.
    fn done(&self, now: Duration) -> bool {
        now.saturating_sub(self.since) >= self.span()
    }

    /// Turn toward `to` from wherever the value is at `now`.
    fn turn(&mut self, to: f32, now: Duration) {
        if (self.to - to).abs() > f32::EPSILON {
            self.from = self.at(now);
            self.to = to;
            self.since = now;
        }
    }
}

/// One slot on its way in or out: the rule whose result it shows, how
/// opaque the slot is, and while it crossfades from another rule's result to
/// this one's, that rule and how much of the mix is this one's (`mix`,
/// rising to 1, when the crossfade is done and the other rule let go).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fading {
    key: RuleKey,
    shown: Leg,
    mix: Option<(RuleKey, Leg)>,
}

impl Fading {
    /// `key`, whose rule fades over `fade`, takes the slot over from the
    /// rule it shows: crossfading from that rule's result while that rule is
    /// still bound, and from where the crossfade had got to when `key` is
    /// the rule it was crossfading away from (turned round part of the way).
    /// A rule no longer bound (a reload binds every rule again under a new
    /// generation) hands over where it was, with no crossfade. A third rule
    /// part of the way through takes over from the one being crossfaded to,
    /// as only two results are kept.
    fn hand_over(
        &mut self,
        key: RuleKey,
        fade: Fade,
        bound: &impl Fn(RuleKey) -> bool,
        now: Duration,
    ) {
        self.mix = match self.mix.take() {
            Some((from, mix)) if from == key => {
                Some((self.key, Leg::rising_from(1.0 - mix.at(now), fade, now)))
            }
            _ if bound(self.key) => Some((self.key, Leg::rising(fade, now))),
            _ => None,
        };
        self.key = key;
    }

    /// Let go of the rule crossfaded from once the crossfade is done, or
    /// once a reload has dropped it.
    fn settle(&mut self, bound: &impl Fn(RuleKey) -> bool, now: Duration) {
        if self
            .mix
            .is_some_and(|(from, mix)| !bound(from) || mix.done(now))
        {
            self.mix = None;
        }
    }

    /// What the slot draws at `now`.
    fn shown(&self, now: Duration) -> Shown {
        Shown {
            key: self.key,
            opacity: self.shown.at(now),
            from: self.mix.map(|(from, mix)| (from, mix.at(now))),
        }
    }

    /// Whether anything of it is still moving at `now`.
    fn moving(&self, now: Duration) -> bool {
        !self.shown.done(now) || self.mix.is_some_and(|(_, mix)| !mix.done(now))
    }
}

/// What a slot draws this pass: the rule whose result it is, and how
/// opaque; and while the slot crossfades to that rule from another one
/// (`from`), that rule, kept in the slot's `Leaving` twin
/// ([`Slot::leaving`]), and how far the crossfade has got, from 0, all the
/// other rule's result, to 1, all this one's.
/// `tests::a_slot_fades_in_as_its_rule_comes_and_out_as_it_goes`,
/// `tests::a_slot_crossfades_from_one_rule_to_the_next`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shown {
    pub(crate) key: RuleKey,
    pub(crate) opacity: f32,
    pub(crate) from: Option<(RuleKey, f32)>,
}

/// Every slot that is fading, or showing a rule that fades, by its owner and
/// slot. `tests`.
#[derive(Debug, Default)]
pub(crate) struct Fades {
    by_slot: HashMap<(Owner, Slot), Fading>,
    /// The slots stepped since the last [`Fades::sweep`].
    stepped: HashSet<(Owner, Slot)>,
}

impl Fades {
    /// What `(owner, slot)` draws this pass at `now`, given the rule it
    /// resolved to (`None` for none) with that rule's fade, and whether a
    /// rule is still bound (`bound`: a reload may have dropped it).
    ///
    /// A rule with no fade is shown whole, at once, and one that stops is
    /// gone at once, as before fades. A rule with one comes in from 0, and
    /// when it stops the slot goes on showing its result, fading to 0, and
    /// is wanted no more once there. Turned round part of the way, it goes
    /// back from where it got to. Another rule with a fade taking the slot
    /// over crossfades from the result the slot showed to its own over its
    /// fade ([`Fading::hand_over`]), the slot's opacity going on as it was;
    /// the same rule bound again by a reload takes it over where the fade
    /// had got to.
    /// `tests::a_slot_fades_in_as_its_rule_comes_and_out_as_it_goes`,
    /// `tests::a_fade_turned_round_goes_back_from_where_it_got_to`,
    /// `tests::a_rule_without_a_fade_comes_and_goes_at_once`,
    /// `tests::a_reloaded_rule_takes_its_slot_over_where_the_fade_had_got`,
    /// `tests::a_slot_crossfades_from_one_rule_to_the_next`,
    /// `tests::a_crossfade_turned_round_goes_back_from_where_it_got_to`,
    /// `tests::a_crossfading_slot_whose_rule_stops_fades_out_whole`.
    pub(crate) fn step(
        &mut self,
        owner: &Owner,
        slot: Slot,
        resolved: Option<(RuleKey, Option<Fade>)>,
        bound: impl Fn(RuleKey) -> bool,
        now: Duration,
    ) -> Option<Shown> {
        // Nothing held and nothing to fade: the slot is its rule, as before
        // fades, and nothing is cloned or kept for it.
        if self.by_slot.is_empty() && !matches!(resolved, Some((_, Some(_)))) {
            return resolved.map(|(key, _)| Shown {
                key,
                opacity: 1.0,
                from: None,
            });
        }
        let at = (owner.clone(), slot);
        match resolved {
            Some((key, None)) => {
                self.by_slot.remove(&at);
                Some(Shown {
                    key,
                    opacity: 1.0,
                    from: None,
                })
            }
            Some((key, Some(fade))) => {
                self.stepped.insert(at.clone());
                let fading = self.by_slot.entry(at).or_insert(Fading {
                    key,
                    shown: Leg::rising(fade, now),
                    mix: None,
                });
                if fading.key != key {
                    fading.hand_over(key, fade, &bound, now);
                }
                fading.shown.fade = fade;
                fading.shown.turn(1.0, now);
                fading.settle(&bound, now);
                Some(fading.shown(now))
            }
            None => {
                let fading = self.by_slot.get_mut(&at)?;
                self.stepped.insert(at.clone());
                if !bound(fading.key) {
                    self.by_slot.remove(&at);
                    return None;
                }
                fading.shown.turn(0.0, now);
                fading.settle(&bound, now);
                if fading.shown.done(now) {
                    self.by_slot.remove(&at);
                    return None;
                }
                Some(fading.shown(now))
            }
        }
    }

    /// Whether a slot is still on its way in or out, or crossfading, at
    /// `now`: the frames that show it are owed, since nothing else on screen
    /// may change to draw them.
    /// `tests::a_slot_fades_in_as_its_rule_comes_and_out_as_it_goes`,
    /// `tests::a_slot_crossfades_from_one_rule_to_the_next`.
    pub(crate) fn moving(&self, now: Duration) -> bool {
        self.by_slot.values().any(|fading| fading.moving(now))
    }

    /// At the end of a pass: keep the slots stepped in it, and of the rest
    /// only those whose owner `keep` says is still there, though not drawn
    /// this pass (a window on a workspace out of sight).
    /// `tests::a_gone_owners_fades_are_dropped`.
    pub(crate) fn sweep(&mut self, mut keep: impl FnMut(&Owner) -> bool) {
        self.by_slot
            .retain(|at, _| self.stepped.contains(at) || keep(&at.0));
        self.stepped.clear();
    }

    /// Forget every slot: no rule is left to fade.
    pub(crate) fn clear(&mut self) {
        self.by_slot.clear();
        self.stepped.clear();
    }

    /// How many slots are held. Nothing in the compositor asks.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_slot.len()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Fades, Shown};
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::{Fade, Origin, RuleKey, Slot};
    use crate::pane::PaneId;

    fn key(index: u32, generation: u32) -> RuleKey {
        RuleKey {
            origin: Origin::Style,
            index,
            generation,
        }
    }

    fn linear(ms: u64) -> Fade {
        Fade {
            duration: Duration::from_millis(ms),
            curve: solium_animation::Curve::Linear,
        }
    }

    fn ms(ms: u64) -> Duration {
        Duration::from_secs(1000) + Duration::from_millis(ms)
    }

    fn owner(pane: u64) -> Owner {
        Owner::Pane(PaneId::from_raw(pane), PaneSlot::Pane)
    }

    fn opacity(shown: Option<Shown>) -> Option<f32> {
        shown.map(|shown| (shown.opacity * 100.0).round() / 100.0)
    }

    /// **A slot fades in as its rule comes and out as it goes**: from 0 to 1
    /// over the fade's 200 ms, then held; and when the rule stops, its result
    /// still shown, from 1 to 0 over 200 ms, and then not at all, and nothing
    /// held for it. Frames are owed while it moves and not once it rests.
    #[test]
    fn a_slot_fades_in_as_its_rule_comes_and_out_as_it_goes() {
        let mut fades = Fades::default();
        let (pane, shade) = (owner(1), Some((key(1, 1), Some(linear(200)))));
        let bound = |_| true;
        let seen: Vec<Option<f32>> = [0, 50, 100, 200, 400]
            .into_iter()
            .map(|at| opacity(fades.step(&pane, Slot::Front, shade, bound, ms(at))))
            .collect();
        assert_eq!(
            seen,
            [Some(0.0), Some(0.25), Some(0.5), Some(1.0), Some(1.0)]
        );
        assert!(fades.moving(ms(150)) && !fades.moving(ms(400)));
        let seen: Vec<Option<f32>> = [1000, 1050, 1150, 1200, 1300]
            .into_iter()
            .map(|at| opacity(fades.step(&pane, Slot::Front, None, bound, ms(at))))
            .collect();
        assert_eq!(seen, [Some(1.0), Some(0.75), Some(0.25), None, None]);
        assert_eq!(fades.len(), 0, "a slot faded out is held no more");
        assert!(!fades.moving(ms(1300)));
        let shown = fades.step(&pane, Slot::Front, shade, bound, ms(2000));
        assert_eq!(shown.map(|shown| shown.key), Some(key(1, 1)));
    }

    /// **A fade turned round goes back from where it got to**, in that share
    /// of its duration: out at 0.75 when its rule stops 150 ms into a 200 ms
    /// fade in, gone 150 ms later; and in again from where the fade out had
    /// got to.
    #[test]
    fn a_fade_turned_round_goes_back_from_where_it_got_to() {
        let mut fades = Fades::default();
        let (pane, shade) = (owner(1), Some((key(1, 1), Some(linear(200)))));
        let bound = |_| true;
        let _ = fades.step(&pane, Slot::Front, shade, bound, ms(0));
        let out: Vec<Option<f32>> = [150, 225, 290, 300]
            .into_iter()
            .map(|at| opacity(fades.step(&pane, Slot::Front, None, bound, ms(at))))
            .collect();
        assert_eq!(out, [Some(0.75), Some(0.38), Some(0.05), None]);
        let _ = fades.step(&pane, Slot::Front, shade, bound, ms(400));
        let _ = fades.step(&pane, Slot::Front, None, bound, ms(500));
        let back: Vec<Option<f32>> = [550, 600]
            .into_iter()
            .map(|at| opacity(fades.step(&pane, Slot::Front, shade, bound, ms(at))))
            .collect();
        assert_eq!(back, [Some(0.25), Some(0.5)]);
    }

    /// **A rule without a fade comes and goes at once**, as every rule did
    /// before fades, and holds nothing.
    #[test]
    fn a_rule_without_a_fade_comes_and_goes_at_once() {
        let mut fades = Fades::default();
        let pane = owner(1);
        let shown = fades.step(
            &pane,
            Slot::Behind,
            Some((key(0, 1), None)),
            |_| true,
            ms(0),
        );
        assert_eq!(
            shown,
            Some(Shown {
                key: key(0, 1),
                opacity: 1.0,
                from: None,
            })
        );
        assert_eq!(fades.step(&pane, Slot::Behind, None, |_| true, ms(1)), None);
        assert_eq!(fades.len(), 0);
        // And one whose fade is 0 ms is there whole from its first frame.
        let instant = Some((key(0, 1), Some(linear(0))));
        assert_eq!(
            opacity(fades.step(&pane, Slot::Behind, instant, |_| true, ms(2))),
            Some(1.0)
        );
        assert_eq!(fades.step(&pane, Slot::Behind, None, |_| true, ms(3)), None);
    }

    /// **A reloaded rule takes its slot over where the fade had got to**: a
    /// reload binds every rule again under a new generation, and the slot
    /// goes on from the opacity it had, rather than fading in again; a rule
    /// gone with the reload takes its slot's result with it at once.
    #[test]
    fn a_reloaded_rule_takes_its_slot_over_where_the_fade_had_got() {
        let mut fades = Fades::default();
        let pane = owner(1);
        let _ = fades.step(
            &pane,
            Slot::Front,
            Some((key(1, 1), Some(linear(200)))),
            |_| true,
            ms(0),
        );
        let _ = fades.step(
            &pane,
            Slot::Front,
            Some((key(1, 1), Some(linear(200)))),
            |_| true,
            ms(300),
        );
        let reloaded = fades.step(
            &pane,
            Slot::Front,
            Some((key(1, 2), Some(linear(200)))),
            |key| key.generation == 2,
            ms(310),
        );
        assert_eq!(
            reloaded,
            Some(Shown {
                key: key(1, 2),
                opacity: 1.0,
                from: None,
            })
        );
        let _ = fades.step(&pane, Slot::Front, None, |_| true, ms(400));
        assert_eq!(
            fades.step(&pane, Slot::Front, None, |key| key.generation == 3, ms(450)),
            None,
            "the reload dropped the rule that was fading out"
        );
    }

    /// **A gone owner's fades are dropped**, and only its own: a slot
    /// stepped this pass is kept, and one that was not is kept while its
    /// owner is there.
    #[test]
    fn a_gone_owners_fades_are_dropped() {
        let mut fades = Fades::default();
        let shade = Some((key(1, 1), Some(linear(200))));
        for pane in [1, 2, 3] {
            let _ = fades.step(&owner(pane), Slot::Front, shade, |_| true, ms(0));
        }
        fades.sweep(|_| false);
        assert_eq!(fades.len(), 3, "every slot was stepped this pass");
        let _ = fades.step(&owner(3), Slot::Front, shade, |_| true, ms(50));
        fades.sweep(|owner| *owner == self::owner(2));
        assert_eq!(fades.len(), 2, "pane 1 is gone, 2 is out of sight");
        assert_eq!(
            opacity(fades.step(&owner(2), Slot::Front, shade, |_| true, ms(100))),
            Some(0.5)
        );
    }

    /// How far a crossfade has got, to the hundredth, and from which rule.
    fn mixed(shown: Option<Shown>) -> Option<(RuleKey, f32)> {
        shown
            .and_then(|shown| shown.from)
            .map(|(from, share)| (from, (share * 100.0).round() / 100.0))
    }

    /// **A slot crossfades from one rule to the next** (#389): a `replace`
    /// on a window with the keyboard and another on one without, each with a
    /// 200 ms fade. Shown whole by the first, the slot taken by the second
    /// shows the second, whole, crossfading from the first's result kept in
    /// its `Leaving` twin over the second's 200 ms, and then the second
    /// alone. Frames are owed while it crossfades and not once it is done.
    #[test]
    fn a_slot_crossfades_from_one_rule_to_the_next() {
        assert_eq!(Slot::Replace.leaving(), Slot::LeavingReplace);
        assert_eq!(Slot::LeavingReplace.leaving(), Slot::LeavingReplace);
        assert_eq!(Slot::LeavingFront.placed(), Slot::Front);
        assert_eq!(Slot::Behind.placed(), Slot::Behind);
        let mut fades = Fades::default();
        let pane = owner(1);
        let (focused, unfocused) = (
            Some((key(0, 1), Some(linear(200)))),
            Some((key(1, 1), Some(linear(200)))),
        );
        let bound = |_| true;
        let _ = fades.step(&pane, Slot::Replace, focused, bound, ms(0));
        let whole = fades.step(&pane, Slot::Replace, focused, bound, ms(500));
        assert_eq!(
            whole,
            Some(Shown {
                key: key(0, 1),
                opacity: 1.0,
                from: None
            })
        );
        let seen: Vec<_> = [1000, 1050, 1100, 1200, 1300]
            .into_iter()
            .map(|at| {
                let shown = fades.step(&pane, Slot::Replace, unfocused, bound, ms(at));
                (shown.map(|shown| shown.key), opacity(shown), mixed(shown))
            })
            .collect();
        assert_eq!(
            seen,
            [
                (Some(key(1, 1)), Some(1.0), Some((key(0, 1), 0.0))),
                (Some(key(1, 1)), Some(1.0), Some((key(0, 1), 0.25))),
                (Some(key(1, 1)), Some(1.0), Some((key(0, 1), 0.5))),
                (Some(key(1, 1)), Some(1.0), None),
                (Some(key(1, 1)), Some(1.0), None),
            ]
        );
        let _ = fades.step(&pane, Slot::Replace, focused, bound, ms(2000));
        assert!(fades.moving(ms(2100)) && !fades.moving(ms(2200)));
    }

    /// **A crossfade turned round goes back from where it got to**, in that
    /// share of its fade: the keyboard back 150 ms into a 200 ms crossfade
    /// away, the slot shows the first rule again, crossfading from the
    /// second at the quarter it had come to, over the 150 ms back; and no
    /// rule is let go of on the way.
    #[test]
    fn a_crossfade_turned_round_goes_back_from_where_it_got_to() {
        let mut fades = Fades::default();
        let pane = owner(1);
        let (focused, unfocused) = (
            Some((key(0, 1), Some(linear(200)))),
            Some((key(1, 1), Some(linear(200)))),
        );
        let bound = |_| true;
        let _ = fades.step(&pane, Slot::Replace, focused, bound, ms(0));
        let _ = fades.step(&pane, Slot::Replace, unfocused, bound, ms(500));
        let away = fades.step(&pane, Slot::Replace, unfocused, bound, ms(650));
        assert_eq!(mixed(away), Some((key(0, 1), 0.75)));
        let back: Vec<_> = [650, 725, 800]
            .into_iter()
            .map(|at| {
                let shown = fades.step(&pane, Slot::Replace, focused, bound, ms(at));
                (shown.map(|shown| shown.key), mixed(shown))
            })
            .collect();
        assert_eq!(
            back,
            [
                (Some(key(0, 1)), Some((key(1, 1), 0.25))),
                (Some(key(0, 1)), Some((key(1, 1), 0.63))),
                (Some(key(0, 1)), None),
            ]
        );
    }

    /// **A crossfading slot whose rule stops fades out whole**: both results
    /// go on being mixed as the slot's opacity falls, and a rule taking the
    /// slot with no fade of its own takes it at once, holding nothing; a
    /// reload that drops the rule crossfaded from ends the crossfade there.
    #[test]
    fn a_crossfading_slot_whose_rule_stops_fades_out_whole() {
        let mut fades = Fades::default();
        let pane = owner(1);
        let (first, second) = (
            Some((key(0, 1), Some(linear(200)))),
            Some((key(1, 1), Some(linear(200)))),
        );
        let bound = |_| true;
        let _ = fades.step(&pane, Slot::Front, first, bound, ms(0));
        let _ = fades.step(&pane, Slot::Front, second, bound, ms(500));
        let out = fades.step(&pane, Slot::Front, None, bound, ms(550));
        assert_eq!(out.map(|shown| shown.key), Some(key(1, 1)));
        assert_eq!(opacity(out), Some(1.0));
        assert_eq!(mixed(out), Some((key(0, 1), 0.25)));
        let out = fades.step(&pane, Slot::Front, None, bound, ms(600));
        assert_eq!(
            (opacity(out), mixed(out)),
            (Some(0.75), Some((key(0, 1), 0.5)))
        );
        let reloaded = fades.step(&pane, Slot::Front, None, |key| key.index == 1, ms(610));
        assert_eq!(
            mixed(reloaded),
            None,
            "the rule crossfaded from was dropped"
        );
        let _ = fades.step(&pane, Slot::Front, first, bound, ms(1000));
        let plain = fades.step(&pane, Slot::Front, Some((key(2, 1), None)), bound, ms(1100));
        assert_eq!(
            plain,
            Some(Shown {
                key: key(2, 1),
                opacity: 1.0,
                from: None
            })
        );
        assert_eq!(fades.len(), 0);
    }
}
