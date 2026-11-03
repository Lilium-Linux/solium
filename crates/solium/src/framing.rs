//! Who draws a window's frame: Solium, from its pane style, or the
//! application itself (#462). `config.frames`, handed over by `sol.frames`.
//!
//! **A client's say is what it asked through `xdg-decoration`, or that it
//! never asked.** The protocol leaves the frame to the client when there is
//! no decoration object, and GTK4 never makes one: it draws a headerbar
//! whatever it is told, so a frame of Solium's on such a window is a second
//! titlebar. So what each client asked is kept on its surface ([`note`]), and the
//! policy turns it into a frame or none ([`Policy::server_side`]) whenever
//! anything that decides it changes: the client asking, the window's first
//! frame, its `app_id`, or the policy itself. `Solium::decide_frame` is that
//! one place.
//!
//! X11 windows are not asked: they say whether they want a frame through
//! their Motif hints, which `xwayland.rs` reads.

use std::sync::Mutex;

use smithay::{
    reexports::{
        wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{
            Mode as KdeMode, OrgKdeKwinServerDecoration,
        },
        wayland_server::protocol::wl_surface::WlSurface,
    },
    wayland::compositor::with_states,
};

use crate::effect::rules::Glob;

/// What a Wayland client said about its frame, through `xdg-decoration`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Asked {
    /// Nothing: it never made a decoration object, so by the protocol it
    /// draws its own. GTK4, and Firefox, never do.
    #[default]
    Nothing,
    /// It made one and stated no preference, or took it back with
    /// `unset_mode`: the compositor's to choose.
    Unset,
    /// It asked to draw its own.
    Client,
    /// It asked for the compositor's.
    Server,
}

/// How a client's say becomes a frame: `frames.policy`, or a rule's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Policy {
    /// Solium's frame for a client that leaves it to the compositor or asks
    /// for it; none for one that asks to draw its own or never says. The
    /// default: no window has two titlebars, and every one that can take
    /// Solium's frame has it.
    #[default]
    Prefer,
    /// Solium's frame on every window, and a client asking to draw its own
    /// is told the compositor draws it. A client that draws its own anyway
    /// has two.
    Force,
    /// The application's own frame unless it asks for Solium's: one that
    /// leaves it to the compositor is offered client-side.
    Allow,
}

impl Policy {
    /// The policy a word names, if it names one.
    pub(crate) fn named(word: &str) -> Option<Self> {
        match word {
            "prefer" => Some(Self::Prefer),
            "force" => Some(Self::Force),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }

    /// Whether Solium draws the frame of a client that said `asked`.
    /// `tests::each_policy_frames_what_it_says_for_each_answer`.
    pub(crate) const fn server_side(self, asked: Asked) -> bool {
        match self {
            Self::Force => true,
            Self::Prefer => matches!(asked, Asked::Unset | Asked::Server),
            Self::Allow => matches!(asked, Asked::Server),
        }
    }
}

/// One rule: windows whose `app_id` matches take `policy` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) app_id: Glob,
    pub(crate) policy: Policy,
}

/// `config.frames`.
/// `script::tests::sol_frames_reads_the_policy_and_its_rules`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) policy: Policy,
    pub(crate) rules: Vec<Rule>,
}

impl Settings {
    /// The policy for an application: the last rule matching its `app_id`,
    /// as the last matching effect rule wins, or `policy`.
    /// `tests::the_last_rule_matching_an_app_id_wins`.
    pub(crate) fn policy_for(&self, app_id: &str) -> Policy {
        self.rules
            .iter()
            .rev()
            .find(|rule| rule.app_id.matches(app_id))
            .map_or(self.policy, |rule| rule.policy)
    }
}

/// What a client asked, kept on its toplevel's surface.
#[derive(Debug, Default)]
struct Said(Mutex<Asked>);

/// A GTK3 client's `org_kde_kwin_server_decoration`, kept on its toplevel's
/// surface, and the mode it was last told: KDE's protocol is how GTK3 asks
/// for a frame (#61), and it is told which it got through the object it made.
#[derive(Debug, Default)]
struct Kde(Mutex<Option<(OrgKdeKwinServerDecoration, Option<KdeMode>)>>);

/// What a KDE `request_mode` asks: `None` is no decorations at all, which is
/// the client's own to draw.
pub(crate) const fn asked_through_kde(mode: KdeMode) -> Asked {
    match mode {
        KdeMode::Server => Asked::Server,
        _ => Asked::Client,
    }
}

/// Remember `surface`'s KDE decoration object, or forget it.
pub(crate) fn note_kde(surface: &WlSurface, decoration: Option<OrgKdeKwinServerDecoration>) {
    with_states(surface, |states| {
        states.data_map.insert_if_missing_threadsafe(Kde::default);
        if let Some(kde) = states.data_map.get::<Kde>()
            && let Ok(mut held) = kde.0.lock()
        {
            *held = decoration.map(|decoration| (decoration, None));
        }
    });
}

/// Tell `surface`'s KDE decoration object which frame it has, once each
/// time that changes: a client that answers each `mode` with a request of
/// its own would otherwise be told again for every answer.
pub(crate) fn tell_kde(surface: &WlSurface, framed: bool) {
    let mode = if framed {
        KdeMode::Server
    } else {
        KdeMode::Client
    };
    with_states(surface, |states| {
        if let Some(kde) = states.data_map.get::<Kde>()
            && let Ok(mut held) = kde.0.lock()
            && let Some((decoration, told)) = held.as_mut()
            && *told != Some(mode)
        {
            decoration.mode(mode);
            *told = Some(mode);
        }
    });
}

/// Remember what the client of `surface` asked about its frame.
pub(crate) fn note(surface: &WlSurface, asked: Asked) {
    with_states(surface, |states| {
        states.data_map.insert_if_missing_threadsafe(Said::default);
        if let Some(said) = states.data_map.get::<Said>()
            && let Ok(mut held) = said.0.lock()
        {
            *held = asked;
        }
    });
}

/// What the client of `surface` asked about its frame: [`Asked::Nothing`]
/// until it says anything.
pub(crate) fn asked(surface: &WlSurface) -> Asked {
    with_states(surface, |states| {
        states
            .data_map
            .get::<Said>()
            .and_then(|said| said.0.lock().ok().map(|held| *held))
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_policy_frames_what_it_says_for_each_answer() {
        let answers = [Asked::Nothing, Asked::Unset, Asked::Client, Asked::Server];
        let framed = |policy: Policy| answers.map(|asked| policy.server_side(asked));
        assert_eq!(
            framed(Policy::Prefer),
            [false, true, false, true],
            "prefer: (never asked, left it to us, asked for its own, asked for ours)"
        );
        assert_eq!(framed(Policy::Force), [true; 4], "force: every one");
        assert_eq!(
            framed(Policy::Allow),
            [false, false, false, true],
            "allow: only one that asks for ours"
        );
    }

    #[test]
    fn the_last_rule_matching_an_app_id_wins() {
        let settings = Settings {
            policy: Policy::Prefer,
            rules: vec![
                Rule {
                    app_id: Glob::parse("org.gnome.*"),
                    policy: Policy::Force,
                },
                Rule {
                    app_id: Glob::parse("org.gnome.Nautilus"),
                    policy: Policy::Allow,
                },
            ],
        };
        assert_eq!(
            [
                settings.policy_for("org.gnome.Nautilus"),
                settings.policy_for("org.gnome.Calculator"),
                settings.policy_for("firefox"),
                settings.policy_for(""),
            ],
            [Policy::Allow, Policy::Force, Policy::Prefer, Policy::Prefer]
        );
    }

    #[test]
    fn the_policy_words_are_the_three_and_nothing_else() {
        assert_eq!(
            ["prefer", "force", "allow", "Prefer", "server", ""].map(Policy::named),
            [
                Some(Policy::Prefer),
                Some(Policy::Force),
                Some(Policy::Allow),
                None,
                None,
                None,
            ]
        );
    }
}
