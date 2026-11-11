//! The first desktop frame held until what it shows is ready (spec C14,
//! §8.3's "pop-ins after the first frame: none"), at most
//! `startup.first_frame.max_ms`. Its one predicate is
//! `Solium::first_frame_ready`; milestone 1's warm-up list (U17) adds its
//! terms there and owns this gate, and the `startup` section, after it.
//! `tests::the_first_frame_is_held_until_ready_then_never_again`.

use std::time::Duration;

use crate::script::{Doc, DocTable};

/// `startup.first_frame`: whether the first frame is held, the longest it
/// is (spec §8.3's cap, 2 s by default), and whether a reload arms the
/// gate again.
/// `tests::hold_and_on_reload_are_obeyed`, `tests::the_hold_is_capped_at_max_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FirstFrame {
    pub(crate) hold: bool,
    pub(crate) max_ms: u64,
    pub(crate) on_reload: bool,
}

impl Default for FirstFrame {
    fn default() -> Self {
        Self {
            hold: true,
            max_ms: 2000,
            on_reload: false,
        }
    }
}

/// The longest `max_ms` may be: a safety cap on a black screen, itself
/// checked (`tests::startup_first_frame_is_read_and_checked`).
const MAX_MS: i64 = 10_000;

/// The keys `startup.first_frame` takes.
const KEYS: [&str; 3] = ["hold", "max_ms", "on_reload"];

/// `startup.first_frame`, read and checked through `Doc`'s own shape: each
/// key refused by name outside its type or bounds, a whole `max_ms` Lua
/// holds as a float (`1e3`) read, and an unknown key named with the nearest
/// known one.
/// `tests::startup_first_frame_is_read_and_checked`.
pub(crate) fn parse(tree: &Doc) -> Result<FirstFrame, String> {
    let shape =
        || "`startup.first_frame` is a table: { hold = true, max_ms = 2000, on_reload = false }";
    let Doc::Table(DocTable { list, fields, .. }) = tree else {
        return Err(shape().to_owned());
    };
    if !list.is_empty() {
        return Err(shape().to_owned());
    }
    let mut config = FirstFrame::default();
    for (key, value) in fields {
        match (key.as_str(), value) {
            ("hold", Doc::Bool(hold)) => config.hold = *hold,
            ("on_reload", Doc::Bool(on)) => config.on_reload = *on,
            ("max_ms", Doc::Int(ms)) if (0..=MAX_MS).contains(ms) => {
                config.max_ms = ms.unsigned_abs();
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "whole and within 0 to 10000, checked first, so exact"
            )]
            ("max_ms", Doc::Num(ms)) if ms.fract() == 0.0 && (0.0..=10_000.0).contains(ms) => {
                config.max_ms = *ms as u64;
            }
            ("max_ms", _) => {
                return Err(
                    "`startup.first_frame.max_ms` is a whole number of ms, 0 to 10000".to_owned(),
                );
            }
            ("hold" | "on_reload", _) => {
                return Err(format!("`startup.first_frame.{key}` is true or false"));
            }
            (other, _) => {
                let meant = solium_effects::spec::nearest(other, &KEYS)
                    .map(|meant| format!("; did you mean `{meant}`?"))
                    .unwrap_or_default();
                return Err(format!(
                    "`{other}` is not a first_frame key: hold, max_ms or on_reload{meant}"
                ));
            }
        }
    }
    Ok(config)
}

/// `sol.startup`'s options: `first_frame` read through [`parse`], with a
/// number in it that is not finite kept, so `max_ms = 1/0` is refused
/// rather than read as the default; any other key refused by name, with
/// the nearest one. `script::tests::sol_startup_reaches_the_compositor`.
pub(crate) fn startup(options: &Doc) -> Result<FirstFrame, String> {
    let mut read = Ok(FirstFrame::default());
    let Doc::Table(table) = options else {
        return read;
    };
    if !table.list.is_empty() || !table.numbered.is_empty() {
        return Err("`startup` takes named keys: first_frame".to_owned());
    }
    for (key, value) in &table.fields {
        if key != "first_frame" {
            let meant = solium_effects::spec::nearest(key, &["first_frame"])
                .map(|meant| format!("; did you mean `{meant}`?"))
                .unwrap_or_default();
            return Err(format!("`{key}` is not a startup key: first_frame{meant}"));
        }
        read = match value.plain() {
            Some(mut tree) => {
                tree.keep_non_finite_fields(value);
                parse(&tree)
            }
            None => parse(&Doc::Bool(false)),
        };
    }
    read
}

/// When the hold began, how many passes it has held, whether it is over for
/// good, whether the cap ended it, and how many passes the dev control
/// `SOLIUM_WARM_INJECT` says the frame is not ready for.
/// `tests::the_first_frame_is_held_until_ready_then_never_again`.
#[derive(Debug, Default)]
pub(crate) struct Warm {
    config: FirstFrame,
    since: Option<Duration>,
    done: bool,
    held: u64,
    capped: bool,
    inject: u64,
}

impl Warm {
    /// `startup.first_frame`, as `sol.startup` last gave it.
    /// `tests::hold_and_on_reload_are_obeyed`.
    pub(crate) fn configure(&mut self, config: FirstFrame) {
        self.config = config;
    }

    /// `SOLIUM_WARM_INJECT`'s passes: the first frame is not ready for that
    /// many held passes of each hold (`dev::warm_inject`; 0, nothing).
    /// `state::tests::an_injected_wait_holds_the_first_frame_for_its_passes`.
    pub(crate) fn inject(&mut self, passes: u64) {
        self.inject = passes;
    }

    /// Whether `SOLIUM_WARM_INJECT`'s passes are over, so the frame may be
    /// ready: `Solium::first_frame_ready` asks it.
    /// `state::tests::an_injected_wait_holds_the_first_frame_for_its_passes`.
    pub(crate) fn injected_passes_over(&self) -> bool {
        self.held >= self.inject
    }

    /// A reload succeeded: with `on_reload`, a gate that has let its frame
    /// through holds again until ready, at most `max_ms` from the next pass
    /// asked. A gate still holding is left as it is, so `max_ms` caps the
    /// clear colour however many reloads come while it shows.
    /// `tests::hold_and_on_reload_are_obeyed`,
    /// `tests::a_reload_during_a_hold_keeps_its_cap`.
    pub(crate) fn reloaded(&mut self) {
        if self.config.on_reload && self.done {
            self.done = false;
            self.since = None;
            self.held = 0;
            self.capped = false;
        }
    }

    /// The gate has let a frame through and holds no more (until a reload
    /// arms it, with `on_reload`).
    /// `tests::the_hold_is_capped_at_max_ms`.
    pub(crate) fn released(&self) -> bool {
        self.done
    }

    /// The gate let its frame through at `max_ms` with what it waited for
    /// still not ready: what `render::hold_first_frame` names in the log.
    /// Not after a frame drawn ready, nor under `hold = false` or
    /// `max_ms = 0`, which wait for nothing.
    /// `tests::the_hold_is_capped_at_max_ms`,
    /// `tests::the_first_frame_is_held_until_ready_then_never_again`.
    pub(crate) fn capped(&self) -> bool {
        self.capped
    }

    /// Whether this pass draws only the clear colour: until `ready`, at most
    /// `max_ms` from the first pass asked, never with `hold = false` or
    /// `max_ms = 0`, and never once released (until a reload arms it, with
    /// `on_reload`). A frame drawn unheld under `hold = false` or
    /// `max_ms = 0` releases it too: it was the first frame, so a reload
    /// that turns the hold on holds nothing unless `on_reload` arms it.
    /// `tests::the_first_frame_is_held_until_ready_then_never_again`,
    /// `tests::the_hold_is_capped_at_max_ms`,
    /// `tests::hold_and_on_reload_are_obeyed`,
    /// `tests::with_nothing_pending_nothing_is_held`.
    pub(crate) fn holding(&mut self, now: Duration, ready: bool) -> bool {
        if self.done {
            return false;
        }
        if !self.config.hold || self.config.max_ms == 0 {
            self.done = true;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if ready || now.saturating_sub(since) >= Duration::from_millis(self.config.max_ms) {
            tracing::debug!(
                passes = self.held,
                ready,
                waited_ms = now.saturating_sub(since).as_millis(),
                "warm-up: first frame after {} passes",
                self.held
            );
            self.done = true;
            self.capped = !ready;
            return false;
        }
        self.held = self.held.saturating_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{FirstFrame, Warm};

    /// **The first frame is held until ready, then never again**: held
    /// while the effects are not ready, released the pass they are, and a
    /// later pass that is not ready (a reload's new rule) draws at once.
    #[test]
    fn the_first_frame_is_held_until_ready_then_never_again() {
        let mut warm = Warm::default();
        let ms = Duration::from_millis;
        assert!(warm.holding(ms(0), false));
        assert!(warm.holding(ms(16), false));
        assert!(!warm.released());
        assert!(!warm.holding(ms(32), true));
        assert!(warm.released());
        assert!(!warm.capped(), "released when ready, and called capped");
        assert!(
            !warm.holding(ms(48), false),
            "held again after it was released"
        );
    }

    /// **The hold is capped at `max_ms`**, 2 s by default (spec §8.3's
    /// cap): what is still not ready then is drawn as it is; `max_ms = 500`
    /// releases at 500 ms, and `max_ms = 0` never holds.
    #[test]
    fn the_hold_is_capped_at_max_ms() {
        for max_ms in [FirstFrame::default().max_ms, 500] {
            let mut warm = Warm::default();
            warm.configure(FirstFrame {
                max_ms,
                ..FirstFrame::default()
            });
            let cap = Duration::from_millis(max_ms);
            assert!(warm.holding(Duration::ZERO, false));
            assert!(
                warm.holding(cap - Duration::from_millis(1), false),
                "max_ms = {max_ms}"
            );
            assert!(!warm.holding(cap, false), "max_ms = {max_ms}");
            assert!(warm.released(), "max_ms = {max_ms}");
            assert!(
                warm.capped(),
                "max_ms = {max_ms}: let through at the cap, not called capped"
            );
        }
        let mut never = Warm::default();
        never.configure(FirstFrame {
            max_ms: 0,
            ..FirstFrame::default()
        });
        assert!(!never.holding(Duration::ZERO, false), "max_ms = 0 held");
        assert!(
            never.released(),
            "a first frame drawn at max_ms = 0 left the gate open"
        );
        assert!(
            !never.capped(),
            "max_ms = 0 called capped, so the log blames the effects"
        );
        never.configure(FirstFrame::default());
        assert!(
            !never.holding(Duration::from_millis(16), false),
            "a reload's max_ms held a session already drawn"
        );
    }

    /// **`hold = false` never holds, and `on_reload = true` holds again
    /// after a reload that is not ready.** The first frame drawn unheld is
    /// the first frame: a reload that turns `hold` on holds nothing, unless
    /// `on_reload` arms the gate.
    #[test]
    fn hold_and_on_reload_are_obeyed() {
        let ms = Duration::from_millis;
        let mut off = Warm::default();
        off.configure(FirstFrame {
            hold: false,
            ..FirstFrame::default()
        });
        assert!(!off.holding(ms(0), false));
        assert!(
            !off.capped(),
            "hold = false called capped, so the log blames the effects"
        );
        off.configure(FirstFrame::default());
        off.reloaded();
        assert!(
            !off.holding(ms(16), false),
            "a reload's hold = true held a session already drawn"
        );
        let mut again = Warm::default();
        again.configure(FirstFrame {
            on_reload: true,
            ..FirstFrame::default()
        });
        assert!(!again.holding(ms(0), true));
        again.reloaded();
        assert!(
            again.holding(ms(100), false),
            "a reload did not arm the gate again"
        );
        assert!(
            again.holding(ms(2099), false),
            "the cap counted from the first pass, not from the reload"
        );
        assert!(!again.holding(ms(2100), false));
        let mut once = Warm::default();
        assert!(!once.holding(ms(0), true));
        once.reloaded();
        assert!(
            !once.holding(ms(100), false),
            "on_reload = false held after a reload"
        );
    }

    /// **A reload during a hold keeps its cap**: `on_reload` arms a gate
    /// that has let its frame through, and a reload while it still holds
    /// changes nothing, so `max_ms` caps the screen's clear colour however
    /// often the configuration is reloaded while it shows (an editor saving
    /// as it types, with auto-reload).
    #[test]
    fn a_reload_during_a_hold_keeps_its_cap() {
        let ms = Duration::from_millis;
        let mut warm = Warm::default();
        warm.configure(FirstFrame {
            on_reload: true,
            ..FirstFrame::default()
        });
        assert!(warm.holding(ms(0), false));
        for at in [500, 1000, 1500, 1999] {
            warm.reloaded();
            assert!(warm.holding(ms(at), false), "released early at {at} ms");
        }
        assert!(
            !warm.holding(ms(2000), false),
            "the reloads during the hold moved its cap"
        );
        assert!(warm.capped());
    }

    /// **`startup.first_frame` is read and checked.**
    #[test]
    fn startup_first_frame_is_read_and_checked() {
        let tree = |lua: &str| crate::script::doc::tests::plain(lua).expect("a table");
        assert_eq!(
            super::parse(&tree("{ hold = false, max_ms = 500, on_reload = true }")),
            Ok(FirstFrame {
                hold: false,
                max_ms: 500,
                on_reload: true
            })
        );
        assert_eq!(
            super::parse(&tree("{ max_ms = 1e3 }")).map(|read| read.max_ms),
            Ok(1000),
            "a whole number Lua holds as a float"
        );
        assert_eq!(super::parse(&tree("{}")), Ok(FirstFrame::default()));
        for refused in ["{ max_ms = 20000 }", "{ max_ms = -1 }", "{ max_ms = 1.5 }"] {
            assert!(
                super::parse(&tree(refused))
                    .expect_err(refused)
                    .contains("0 to 10000"),
                "{refused}"
            );
        }
        assert!(
            super::parse(&tree("{ hld = true }"))
                .expect_err("refused")
                .contains("hold, max_ms or on_reload; did you mean `hold`?")
        );
        assert!(
            super::parse(&tree("{ hold = 1 }"))
                .expect_err("refused")
                .contains("true or false")
        );
        assert!(
            super::parse(&tree("{ true }"))
                .expect_err("refused")
                .contains("is a table")
        );
    }

    /// **With nothing pending nothing is held**: the first pass is ready, so
    /// a desktop with no effects draws its first frame as today (§8.4).
    #[test]
    fn with_nothing_pending_nothing_is_held() {
        assert!(!Warm::default().holding(Duration::ZERO, true));
    }
}
