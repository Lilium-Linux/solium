//! The notifications themselves, as a pure state machine (03 §3.2.11, "Two
//! lifetimes"): what arrived, what is on screen, what was kept, and what the
//! protocol must be told. No bus, no Qt and no clock of its own: time comes in
//! as the compositor clock's `now`, and what must be said goes out as
//! [`Effect`]s.

use std::time::Duration;

use super::content::{Body, Image};

/// How urgent the sender says it is (`urgency` hint, a byte).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl Urgency {
    pub(crate) fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }
}

/// Why a notification closed, as `NotificationClosed` numbers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    /// Its banner ended and nothing kept it.
    Expired = 1,
    /// The user dismissed it, or invoked an action of one that is not
    /// resident.
    Dismissed = 2,
    /// The application asked (`CloseNotification`).
    Closed = 3,
    /// Anything else: pushed out of a full history, or dropped by policy.
    Undefined = 4,
}

impl Reason {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Dismissed => "dismissed",
            Self::Closed => "closed",
            Self::Undefined => "undefined",
        }
    }
}

/// One `Notify`, made safe ([`super::content`]) and given its id by the server
/// thread, which answered the call with it before any of this ran.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Incoming {
    pub(crate) id: u32,
    /// The sender's unique name on the bus, `:1.42`, or empty for one of the
    /// shell's own (`sol.notify`).
    pub(crate) sender: String,
    /// The sender's process, as the bus says, when it said.
    pub(crate) pid: Option<u32>,
    pub(crate) app_name: String,
    /// The `desktop-entry` hint.
    pub(crate) desktop_entry: String,
    /// `app_icon` when it names a theme icon.
    pub(crate) app_icon: String,
    pub(crate) summary: String,
    pub(crate) body: Body,
    /// `image-data`, or the `image-path` / path `app_icon` read and decoded.
    pub(crate) image: Option<Image>,
    pub(crate) urgency: Urgency,
    pub(crate) category: String,
    /// `(key, label)`, in the sender's order, `default` included.
    pub(crate) actions: Vec<(String, String)>,
    pub(crate) resident: bool,
    pub(crate) transient: bool,
    /// `expire_timeout`: -1 for the server's default, 0 for never, else ms.
    pub(crate) expire_timeout: i32,
    /// Milliseconds since the epoch, for showing when it came.
    pub(crate) received: u64,
}

/// What the policy decided for one arrival (`sol.on("notification")`, or
/// [`Decision::default_for`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Decision {
    /// Show a banner.
    pub(crate) banner: bool,
    /// Keep it in the history once its banner ends.
    pub(crate) keep: bool,
}

impl Decision {
    /// The shipped rule: a banner unless do-not-disturb is on, where only a
    /// critical one gets one; kept unless the sender said it is transient.
    /// `do_not_disturb_banners_only_critical_ones_and_keeps_the_rest`.
    pub(crate) fn default_for(incoming: &Incoming, dnd: bool) -> Self {
        Self {
            banner: !dnd || incoming.urgency == Urgency::Critical,
            keep: !incoming.transient,
        }
    }
}

/// How long banners last when the sender leaves it to the server, per
/// urgency. `None` is never: a critical banner stays until it is dealt with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Timeouts {
    pub(crate) low: Option<Duration>,
    pub(crate) normal: Option<Duration>,
    pub(crate) critical: Option<Duration>,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            low: Some(Duration::from_millis(5000)),
            normal: Some(Duration::from_millis(7000)),
            critical: None,
        }
    }
}

/// What the configuration says (`sol.notifications{ ... }`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) timeouts: Timeouts,
    /// How many kept notifications the history holds; 0 keeps none.
    pub(crate) history_max: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeouts: Timeouts::default(),
            history_max: 200,
        }
    }
}

/// Where a notification is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// On screen. `deadline` is when the banner ends on the compositor clock,
    /// `None` while it is paused (held, locked, idle) or never ends;
    /// `remaining` is what is left of it while paused, `None` for never.
    Banner {
        deadline: Option<Duration>,
        remaining: Option<Duration>,
    },
    /// Its banner ended, or it never had one; kept in the history.
    Stored,
}

/// One live notification.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub(crate) incoming: Incoming,
    /// 1 when it arrived, and one more each time it was replaced.
    pub(crate) revision: u32,
    pub(crate) shown: Shown,
    /// A pointer or a finger is on its banner.
    pub(crate) held: bool,
    pub(crate) keep: bool,
}

impl Item {
    pub(crate) fn has_default(&self) -> bool {
        self.incoming
            .actions
            .iter()
            .any(|(key, _)| key == "default")
    }
}

/// The most banners on screen at once. Past it the oldest banner ends early,
/// as if its time were up: critical ones, ones that never expire and ones
/// that arrive while locked would otherwise pile up without end.
/// `banners_past_the_cap_end_oldest_first`.
pub(crate) const MAX_BANNERS: usize = 8;

/// What the protocol, and Lua, must be told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// `NotificationClosed(id, reason)`.
    Closed { id: u32, reason: Reason },
    /// `ActivationToken` then `ActionInvoked(id, key)`.
    Invoked { id: u32, key: String },
}

/// Every live notification, oldest first, and the switches that govern them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Notifications {
    items: Vec<Item>,
    dnd: bool,
    /// Locked, or idle: no banner counts down.
    paused: bool,
    settings: Settings,
}

impl Notifications {
    /// New settings: a shorter history drops the oldest kept ones.
    pub(crate) fn configure(&mut self, settings: Settings) -> Vec<Effect> {
        self.settings = settings;
        self.trim_history()
    }

    pub(crate) fn dnd(&self) -> bool {
        self.dnd
    }

    pub(crate) fn set_dnd(&mut self, on: bool) {
        self.dnd = on;
    }

    #[cfg(test)]
    pub(crate) fn items(&self) -> &[Item] {
        &self.items
    }

    pub(crate) fn get(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|item| item.incoming.id == id)
    }

    /// On screen now, newest first.
    pub(crate) fn banners(&self) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .rev()
            .filter(|item| matches!(item.shown, Shown::Banner { .. }))
    }

    /// Kept, newest first.
    pub(crate) fn history(&self) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .rev()
            .filter(|item| item.shown == Shown::Stored)
    }

    /// How long a banner for this one lasts, `None` for never.
    fn lifetime(&self, incoming: &Incoming) -> Option<Duration> {
        if incoming.urgency == Urgency::Critical {
            // Never on its own, whatever the sender asked: a critical one
            // waits to be dealt with.
            // `a_critical_banner_never_times_out_even_if_asked_to`.
            return self.settings.timeouts.critical;
        }
        match incoming.expire_timeout {
            0 => None,
            ms if ms > 0 => Some(Duration::from_millis(u64::from(ms.unsigned_abs()))),
            _ => match incoming.urgency {
                Urgency::Low => self.settings.timeouts.low,
                _ => self.settings.timeouts.normal,
            },
        }
    }

    fn banner(&self, incoming: &Incoming, now: Duration) -> Shown {
        let remaining = self.lifetime(incoming);
        Shown::Banner {
            deadline: if self.paused {
                None
            } else {
                remaining.map(|left| now + left)
            },
            remaining,
        }
    }

    /// One arrival, with what the policy decided. A replacement (an id
    /// already live) takes the old one's place, keeps its position, counts a
    /// revision and starts its timer again; the server thread has already
    /// checked the sender may replace it.
    /// `a_replacement_takes_the_place_of_the_old_one_and_resets_its_timer`,
    /// `a_notification_neither_shown_nor_kept_closes_at_once`.
    pub(crate) fn arrive(
        &mut self,
        incoming: Incoming,
        decision: Decision,
        now: Duration,
    ) -> Vec<Effect> {
        let keep = decision.keep && !incoming.transient && self.settings.history_max > 0;
        if !decision.banner && !keep {
            let id = incoming.id;
            self.items.retain(|item| item.incoming.id != id);
            return vec![Effect::Closed {
                id,
                reason: Reason::Expired,
            }];
        }
        let shown = if decision.banner {
            self.banner(&incoming, now)
        } else {
            Shown::Stored
        };
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.incoming.id == incoming.id)
        {
            item.incoming = incoming;
            item.revision += 1;
            item.shown = shown;
            item.keep = keep;
            item.held = false;
        } else {
            self.items.push(Item {
                incoming,
                revision: 1,
                shown,
                held: false,
                keep,
            });
        }
        let mut effects = self.cap_banners();
        effects.extend(self.trim_history());
        effects
    }

    /// The oldest banners past [`MAX_BANNERS`] end: kept ones go to the
    /// history, the rest close as expired.
    fn cap_banners(&mut self) -> Vec<Effect> {
        let banners: Vec<u32> = self
            .items
            .iter()
            .filter(|item| matches!(item.shown, Shown::Banner { .. }))
            .map(|item| item.incoming.id)
            .collect();
        let over = banners.len().saturating_sub(MAX_BANNERS);
        let mut effects = Vec::new();
        for id in &banners[..over] {
            let Some(item) = self.items.iter_mut().find(|item| item.incoming.id == *id) else {
                continue;
            };
            if item.keep {
                item.shown = Shown::Stored;
                item.held = false;
            } else {
                effects.extend(self.remove(*id, Reason::Expired));
            }
        }
        effects
    }

    /// When the next banner ends, if one will.
    #[cfg(test)]
    pub(crate) fn next_deadline(&self) -> Option<Duration> {
        self.items
            .iter()
            .filter_map(|item| match item.shown {
                Shown::Banner { deadline, .. } => deadline,
                Shown::Stored => None,
            })
            .min()
    }

    /// Every banner whose time is up ends: kept ones move to the history and
    /// say nothing; the rest close as expired.
    /// `a_banner_that_times_out_moves_to_the_history_and_says_nothing`,
    /// `a_transient_one_closes_as_expired_when_its_banner_ends`.
    pub(crate) fn expire(&mut self, now: Duration) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut gone = Vec::new();
        for item in &mut self.items {
            let Shown::Banner {
                deadline: Some(deadline),
                ..
            } = item.shown
            else {
                continue;
            };
            if deadline > now {
                continue;
            }
            if item.keep {
                item.shown = Shown::Stored;
                item.held = false;
            } else {
                gone.push(item.incoming.id);
                effects.push(Effect::Closed {
                    id: item.incoming.id,
                    reason: Reason::Expired,
                });
            }
        }
        self.items.retain(|item| !gone.contains(&item.incoming.id));
        effects.extend(self.trim_history());
        effects
    }

    /// A pointer or a finger rests on a banner: its time stops.
    /// `holding_a_banner_stops_its_time_and_letting_go_gives_back_what_was_left`.
    pub(crate) fn hold(&mut self, id: u32, now: Duration) {
        if let Some(item) = self.items.iter_mut().find(|item| item.incoming.id == id) {
            item.held = true;
            if let Shown::Banner {
                deadline: Some(deadline),
                ..
            } = item.shown
            {
                item.shown = Shown::Banner {
                    deadline: None,
                    remaining: Some(deadline.saturating_sub(now)),
                };
            }
        }
    }

    /// It is let go: what was left of its time runs again, unless the
    /// session is paused.
    pub(crate) fn release(&mut self, id: u32, now: Duration) {
        let paused = self.paused;
        if let Some(item) = self.items.iter_mut().find(|item| item.incoming.id == id) {
            item.held = false;
            if let Shown::Banner {
                deadline: None,
                remaining: Some(left),
            } = item.shown
                && !paused
            {
                item.shown = Shown::Banner {
                    deadline: Some(now + left),
                    remaining: Some(left),
                };
            }
        }
    }

    /// Locked or idle (`true`), or back (`false`): every banner's time stops,
    /// or runs on from where it stopped, held ones excepted.
    /// `a_locked_or_idle_session_stops_every_banner_s_time`.
    pub(crate) fn set_paused(&mut self, paused: bool, now: Duration) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        for item in &mut self.items {
            match item.shown {
                Shown::Banner {
                    deadline: Some(deadline),
                    ..
                } if paused => {
                    item.shown = Shown::Banner {
                        deadline: None,
                        remaining: Some(deadline.saturating_sub(now)),
                    };
                }
                Shown::Banner {
                    deadline: None,
                    remaining: Some(left),
                } if !paused && !item.held => {
                    item.shown = Shown::Banner {
                        deadline: Some(now + left),
                        remaining: Some(left),
                    };
                }
                _ => {}
            }
        }
    }

    fn remove(&mut self, id: u32, reason: Reason) -> Vec<Effect> {
        let before = self.items.len();
        self.items.retain(|item| item.incoming.id != id);
        if self.items.len() == before {
            return Vec::new();
        }
        vec![Effect::Closed { id, reason }]
    }

    /// The user dismissed it, banner or history alike.
    pub(crate) fn dismiss(&mut self, id: u32) -> Vec<Effect> {
        self.remove(id, Reason::Dismissed)
    }

    /// Everything dismissed, oldest first.
    pub(crate) fn dismiss_all(&mut self) -> Vec<Effect> {
        let ids: Vec<u32> = self.items.iter().map(|item| item.incoming.id).collect();
        ids.into_iter().flat_map(|id| self.dismiss(id)).collect()
    }

    /// The application closed it (`CloseNotification`); the server thread
    /// has checked it may.
    pub(crate) fn close_by_app(&mut self, id: u32) -> Vec<Effect> {
        self.remove(id, Reason::Closed)
    }

    /// An action invoked: `default` (a click on the body) only when the
    /// sender offered one, any other key only when it is among its actions.
    /// One that is not resident then closes, as dismissed.
    /// `invoking_an_action_closes_a_notification_that_is_not_resident`,
    /// `an_action_the_sender_never_offered_is_not_invoked`.
    pub(crate) fn invoke(&mut self, id: u32, key: &str) -> Vec<Effect> {
        let Some(item) = self.get(id) else {
            return Vec::new();
        };
        if !item.incoming.actions.iter().any(|(each, _)| each == key) {
            return Vec::new();
        }
        let resident = item.incoming.resident;
        let mut effects = vec![Effect::Invoked {
            id,
            key: key.to_owned(),
        }];
        if !resident {
            effects.extend(self.remove(id, Reason::Dismissed));
        }
        effects
    }

    /// The oldest kept ones past the history's size go.
    /// `a_full_history_drops_its_oldest`.
    fn trim_history(&mut self) -> Vec<Effect> {
        let stored: Vec<u32> = self
            .items
            .iter()
            .filter(|item| item.shown == Shown::Stored)
            .map(|item| item.incoming.id)
            .collect();
        let over = stored.len().saturating_sub(self.settings.history_max);
        stored[..over]
            .iter()
            .flat_map(|&id| self.remove(id, Reason::Undefined))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn incoming(id: u32) -> Incoming {
        Incoming {
            id,
            sender: ":1.7".to_owned(),
            app_name: "Mail".to_owned(),
            summary: format!("message {id}"),
            expire_timeout: -1,
            actions: vec![
                ("default".to_owned(), String::new()),
                ("reply".to_owned(), "Reply".to_owned()),
            ],
            ..Incoming::default()
        }
    }

    const SHOW: Decision = Decision {
        banner: true,
        keep: true,
    };

    fn banner_ids(model: &Notifications) -> Vec<u32> {
        model.banners().map(|item| item.incoming.id).collect()
    }

    fn history_ids(model: &Notifications) -> Vec<u32> {
        model.history().map(|item| item.incoming.id).collect()
    }

    #[test]
    fn a_banner_that_times_out_moves_to_the_history_and_says_nothing() {
        let mut model = Notifications::default();
        assert!(model.arrive(incoming(1), SHOW, ms(0)).is_empty());
        assert_eq!(banner_ids(&model), [1]);
        assert_eq!(model.next_deadline(), Some(ms(7000)));
        assert!(model.expire(ms(6999)).is_empty());
        assert_eq!(banner_ids(&model), [1]);
        assert!(model.expire(ms(7000)).is_empty());
        assert!(banner_ids(&model).is_empty());
        assert_eq!(history_ids(&model), [1]);
        assert_eq!(model.next_deadline(), None);
    }

    #[test]
    fn a_transient_one_closes_as_expired_when_its_banner_ends() {
        let mut model = Notifications::default();
        let mut transient = incoming(1);
        transient.transient = true;
        model.arrive(transient, SHOW, ms(0));
        assert_eq!(
            model.expire(ms(7000)),
            [Effect::Closed {
                id: 1,
                reason: Reason::Expired
            }]
        );
        assert!(model.items().is_empty());
    }

    #[test]
    fn the_sender_s_timeout_and_never_are_honoured_low_is_shorter() {
        let mut model = Notifications::default();
        let mut short = incoming(1);
        short.expire_timeout = 1500;
        let mut never = incoming(2);
        never.expire_timeout = 0;
        let mut low = incoming(3);
        low.urgency = Urgency::Low;
        model.arrive(short, SHOW, ms(0));
        model.arrive(never, SHOW, ms(0));
        model.arrive(low, SHOW, ms(0));
        model.expire(ms(1500));
        assert_eq!(banner_ids(&model), [3, 2]);
        model.expire(ms(5000));
        assert_eq!(banner_ids(&model), [2]);
        model.expire(ms(1_000_000));
        assert_eq!(banner_ids(&model), [2]);
    }

    #[test]
    fn a_critical_banner_never_times_out_even_if_asked_to() {
        let mut model = Notifications::default();
        let mut critical = incoming(1);
        critical.urgency = Urgency::Critical;
        critical.expire_timeout = 1000;
        model.arrive(critical, SHOW, ms(0));
        assert_eq!(model.next_deadline(), None);
        model.expire(ms(1_000_000));
        assert_eq!(banner_ids(&model), [1]);
    }

    #[test]
    fn a_replacement_takes_the_place_of_the_old_one_and_resets_its_timer() {
        let mut model = Notifications::default();
        model.arrive(incoming(1), SHOW, ms(0));
        model.arrive(incoming(2), SHOW, ms(10));
        let mut replacement = incoming(1);
        replacement.summary = "50%".to_owned();
        model.arrive(replacement, SHOW, ms(5000));
        let item = model.get(1).expect("still there");
        assert_eq!(item.revision, 2);
        assert_eq!(item.incoming.summary, "50%");
        // Oldest first underneath: the replaced one keeps its place.
        assert_eq!(
            model
                .items()
                .iter()
                .map(|item| item.incoming.id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        model.expire(ms(7010));
        assert_eq!(banner_ids(&model), [1], "the replaced one runs from 5000");
        model.expire(ms(12_000));
        assert!(banner_ids(&model).is_empty());
    }

    #[test]
    fn holding_a_banner_stops_its_time_and_letting_go_gives_back_what_was_left() {
        let mut model = Notifications::default();
        model.arrive(incoming(1), SHOW, ms(0));
        model.hold(1, ms(5000));
        assert!(model.get(1).expect("there").held);
        assert_eq!(model.next_deadline(), None);
        model.expire(ms(60_000));
        assert_eq!(banner_ids(&model), [1]);
        model.release(1, ms(60_000));
        assert_eq!(model.next_deadline(), Some(ms(62_000)));
    }

    #[test]
    fn a_locked_or_idle_session_stops_every_banner_s_time() {
        let mut model = Notifications::default();
        model.arrive(incoming(1), SHOW, ms(0));
        model.set_paused(true, ms(1000));
        model.arrive(incoming(2), SHOW, ms(2000));
        assert_eq!(model.next_deadline(), None);
        model.expire(ms(100_000));
        assert_eq!(banner_ids(&model), [2, 1]);
        model.set_paused(false, ms(100_000));
        assert_eq!(model.next_deadline(), Some(ms(106_000)));
        model.expire(ms(106_000));
        assert_eq!(banner_ids(&model), [2]);
        model.expire(ms(107_000));
        assert!(banner_ids(&model).is_empty());
    }

    #[test]
    fn a_held_banner_stays_stopped_when_the_session_comes_back() {
        let mut model = Notifications::default();
        model.arrive(incoming(1), SHOW, ms(0));
        model.hold(1, ms(1000));
        model.set_paused(true, ms(2000));
        model.set_paused(false, ms(3000));
        assert_eq!(model.next_deadline(), None);
        model.release(1, ms(4000));
        assert_eq!(model.next_deadline(), Some(ms(10_000)));
    }

    #[test]
    fn do_not_disturb_banners_only_critical_ones_and_keeps_the_rest() {
        let mut model = Notifications::default();
        model.set_dnd(true);
        let normal = incoming(1);
        let decision = Decision::default_for(&normal, model.dnd());
        model.arrive(normal, decision, ms(0));
        let mut critical = incoming(2);
        critical.urgency = Urgency::Critical;
        let decision = Decision::default_for(&critical, model.dnd());
        model.arrive(critical, decision, ms(0));
        assert_eq!(banner_ids(&model), [2]);
        assert_eq!(history_ids(&model), [1]);
    }

    #[test]
    fn a_notification_neither_shown_nor_kept_closes_at_once() {
        let mut model = Notifications::default();
        let effects = model.arrive(
            incoming(1),
            Decision {
                banner: false,
                keep: false,
            },
            ms(0),
        );
        assert_eq!(
            effects,
            [Effect::Closed {
                id: 1,
                reason: Reason::Expired
            }]
        );
        assert!(model.items().is_empty());
    }

    #[test]
    fn dismissing_closes_it_as_dismissed_and_an_app_close_as_closed() {
        let mut model = Notifications::default();
        model.arrive(incoming(1), SHOW, ms(0));
        model.arrive(incoming(2), SHOW, ms(0));
        assert_eq!(
            model.dismiss(1),
            [Effect::Closed {
                id: 1,
                reason: Reason::Dismissed
            }]
        );
        assert_eq!(
            model.close_by_app(2),
            [Effect::Closed {
                id: 2,
                reason: Reason::Closed
            }]
        );
        assert!(model.dismiss(1).is_empty(), "gone already");
    }

    #[test]
    fn invoking_an_action_closes_a_notification_that_is_not_resident() {
        let mut model = Notifications::default();
        model.arrive(incoming(1), SHOW, ms(0));
        let mut resident = incoming(2);
        resident.resident = true;
        model.arrive(resident, SHOW, ms(0));
        assert_eq!(
            model.invoke(1, "default"),
            [
                Effect::Invoked {
                    id: 1,
                    key: "default".to_owned()
                },
                Effect::Closed {
                    id: 1,
                    reason: Reason::Dismissed
                }
            ]
        );
        assert_eq!(
            model.invoke(2, "reply"),
            [Effect::Invoked {
                id: 2,
                key: "reply".to_owned()
            }]
        );
        assert!(model.get(2).is_some());
    }

    #[test]
    fn an_action_the_sender_never_offered_is_not_invoked() {
        let mut model = Notifications::default();
        let mut plain = incoming(1);
        plain.actions.clear();
        model.arrive(plain, SHOW, ms(0));
        assert!(model.invoke(1, "default").is_empty());
        assert!(model.invoke(1, "reply").is_empty());
        assert!(model.get(1).is_some());
        assert!(!model.get(1).expect("there").has_default());
    }

    #[test]
    fn banners_past_the_cap_end_oldest_first() {
        let mut model = Notifications::default();
        for id in 1..=(MAX_BANNERS as u32 + 2) {
            let mut critical = incoming(id);
            critical.urgency = Urgency::Critical;
            model.arrive(critical, SHOW, ms(0));
        }
        assert_eq!(banner_ids(&model).len(), MAX_BANNERS);
        assert_eq!(
            history_ids(&model),
            [2, 1],
            "the oldest two went to the history"
        );
        let mut transient = incoming(100);
        transient.transient = true;
        transient.urgency = Urgency::Critical;
        let effects = model.arrive(transient, SHOW, ms(0));
        assert_eq!(
            effects,
            [],
            "the oldest banner (kept) moved to the history; the new transient one shows"
        );
    }

    #[test]
    fn a_full_history_drops_its_oldest() {
        let mut model = Notifications::default();
        model.configure(Settings {
            history_max: 2,
            ..Settings::default()
        });
        let store = Decision {
            banner: false,
            keep: true,
        };
        assert!(model.arrive(incoming(1), store, ms(0)).is_empty());
        assert!(model.arrive(incoming(2), store, ms(0)).is_empty());
        assert_eq!(
            model.arrive(incoming(3), store, ms(0)),
            [Effect::Closed {
                id: 1,
                reason: Reason::Undefined
            }]
        );
        assert_eq!(history_ids(&model), [3, 2]);
        assert_eq!(
            model.configure(Settings {
                history_max: 1,
                ..Settings::default()
            }),
            [Effect::Closed {
                id: 2,
                reason: Reason::Undefined
            }]
        );
    }

    #[test]
    fn with_no_history_a_banner_that_ends_closes_as_expired() {
        let mut model = Notifications::default();
        model.configure(Settings {
            history_max: 0,
            ..Settings::default()
        });
        model.arrive(incoming(1), SHOW, ms(0));
        assert_eq!(
            model.expire(ms(7000)),
            [Effect::Closed {
                id: 1,
                reason: Reason::Expired
            }]
        );
    }
}
