//! Who draws a window's frame, `frames` (#462), against real clients
//! that say each thing `xdg-decoration` lets them say, or nothing.

use super::*;
use crate::framing::{Policy, Rule, Settings};
use zxdg_toplevel_decoration_v1::Mode as Asks;

/// What a test client says about its frame.
#[derive(Clone, Copy)]
enum Says {
    /// No decoration object: GTK4, Firefox.
    Nothing,
    /// An object and no preference: Qt.
    Unset,
    /// An object and a mode asked for.
    Mode(Asks),
    /// KDE's server-decoration object and a mode asked for, as
    /// GTK3 asks (#61).
    Kde(org_kde_kwin_server_decoration::Mode),
}

const EVERY: [Says; 4] = [
    Says::Nothing,
    Says::Unset,
    Says::Mode(Asks::ClientSide),
    Says::Mode(Asks::ServerSide),
];

/// A window whose client says `says`, named `app_id`, shown: its
/// first buffer committed, so its first frame has decided it.
fn open(desk: &mut Desk, says: Says, app_id: &str) -> crate::pane::PaneId {
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
    let manager = desk
        .client
        .decoration
        .clone()
        .expect("zxdg_decoration_manager_v1 bound");
    let before: Vec<Window> = desk.state.space.elements().cloned().collect();
    let surface = compositor.create_surface(&desk.qh, ());
    let xdg = wm_base.get_xdg_surface(&surface, &desk.qh, ());
    let toplevel = xdg.get_toplevel(&desk.qh, ());
    toplevel.set_app_id(app_id.to_owned());
    match says {
        Says::Nothing => {}
        Says::Unset => {
            let _decoration = manager.get_toplevel_decoration(&toplevel, &desk.qh, ());
        }
        Says::Mode(mode) => {
            manager
                .get_toplevel_decoration(&toplevel, &desk.qh, ())
                .set_mode(mode);
        }
        Says::Kde(mode) => {
            let kde = desk
                .client
                .kde_decoration
                .clone()
                .expect("org_kde_kwin_server_decoration_manager bound");
            kde.create(&surface, &desk.qh, ()).request_mode(mode);
        }
    }
    desk.pump();
    commit_buffer(&desk.client, &desk.qh, &surface, 300, 200);
    desk.pump();
    desk.state.sync_panes();
    desk.pump();
    let window = desk
        .state
        .space
        .elements()
        .find(|window| !before.contains(window))
        .cloned()
        .expect("new_toplevel mapped a window");
    desk.state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane")
}

/// A desk whose windows Solium frames in `top` when it frames them:
/// the fixture's own style is `none`, which frames nothing.
fn framed_desk() -> Desk {
    let mut desk = Desk::new();
    desk.state
        .decorations
        .set_style(&mut desk.state.panes, Some("top".into()));
    desk
}

/// Which frame a pane has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameKind {
    /// The style's whole frame, its bar included.
    Whole,
    /// The style's client-side frame: the application draws its
    /// own titlebar.
    ClientSide,
    /// None at all.
    Bare,
}
use self::FrameKind::{Bare, ClientSide, Whole};

/// The frame the pane has; `None` still undecided -- which no
/// shown window may be.
fn framed(desk: &Desk, pane: crate::pane::PaneId) -> Option<FrameKind> {
    match desk.state.panes.get(pane).map(Pane::frame) {
        Some(crate::pane::Frame::Styled(decoration)) => Some(if decoration.client_side() {
            ClientSide
        } else {
            Whole
        }),
        Some(crate::pane::Frame::None) => Some(Bare),
        Some(crate::pane::Frame::Pending) | None => None,
    }
}

/// `sol.frames`, as a script's dispatch carries it out.
fn frames(desk: &mut Desk, settings: Settings) {
    desk.state.apply(crate::script::Outcome {
        commands: vec![crate::script::Command::Frames(settings)],
        ..crate::script::Outcome::default()
    });
}

fn policy(policy: Policy) -> Settings {
    Settings {
        policy,
        rules: Vec::new(),
    }
}

/// Each policy against each answer, a fresh desk for each:
/// `framing::tests::each_policy_frames_what_it_says_for_each_answer`
/// asked of real clients, so the answer is what the window is
/// drawn with and not only what the table says.
#[test]
fn each_policy_frames_real_clients_as_it_says() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        for (chosen, expected) in [
            (Policy::Prefer, [ClientSide, Whole, ClientSide, Whole]),
            (Policy::Force, [Whole; 4]),
            (Policy::Allow, [ClientSide, ClientSide, ClientSide, Whole]),
        ] {
            let seen = EVERY.map(|says| {
                let mut desk = framed_desk();
                desk.state.frames = policy(chosen);
                let pane = open(&mut desk, says, "org.example.App");
                framed(&desk, pane)
            });
            assert_eq!(
                seen,
                expected.map(Some),
                "{chosen:?}: (never asked, left it to us, asked for its own, asked for ours)"
            );
        }
    });
}

/// **A client that never says draws its own titlebar by default,
/// inside the style's client-side frame**: no bar of Solium's over
/// its own, and only the style's `clientSideInsets` reserved --
/// none for `top`, a bar style, and the border on all four sides
/// for `border`, which has no bar to leave out.
#[test]
fn a_client_drawn_window_is_framed_with_the_styles_client_side_insets() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = framed_desk();
        assert_eq!(desk.state.frames, Settings::default());
        let pane = open(&mut desk, Says::Nothing, "org.gnome.Calculator");
        assert_eq!(
            (framed(&desk, pane), desk.state.insets_of(pane)),
            (Some(ClientSide), crate::decoration::Insets::NONE),
            "under top"
        );
        let mut desk = framed_desk();
        desk.state
            .decorations
            .set_style(&mut desk.state.panes, Some("border".into()));
        let pane = open(&mut desk, Says::Nothing, "org.gnome.Calculator");
        assert_eq!(
            (framed(&desk, pane), desk.state.insets_of(pane)),
            (
                Some(ClientSide),
                crate::decoration::Insets {
                    top: 3,
                    right: 3,
                    bottom: 3,
                    left: 3,
                }
            ),
            "under border"
        );
        let whole = open(&mut desk, Says::Unset, "qt");
        assert_eq!(framed(&desk, whole), Some(Whole));
    });
}

/// **Under `pane = "none"` no window is framed, whatever its
/// application says.**
#[test]
fn under_no_style_every_window_is_bare() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = Desk::new();
        let seen = EVERY.map(|says| {
            let pane = open(&mut desk, says, "app");
            framed(&desk, pane)
        });
        assert_eq!(seen, [Some(Bare); 4]);
    });
}

/// **A rule's `client_side` matches the windows framed
/// client-side**, and `sol.windows()` says which they are.
#[test]
fn client_side_is_a_window_fact_for_rules_and_scripts() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = framed_desk();
        let own = open(&mut desk, Says::Nothing, "gtk");
        let ours = open(&mut desk, Says::Unset, "qt");
        let fact = |desk: &Desk, pane| {
            desk.state
                .window_facts(pane, crate::effect::rules::Keys::default())
                .client_side
        };
        assert_eq!((fact(&desk, own), fact(&desk, ours)), (true, false));
        let listed: Vec<(u64, bool)> = desk
            .state
            .snapshot()
            .windows
            .iter()
            .map(|window| (window.id, window.client_side))
            .collect();
        assert!(
            listed.contains(&(own.get(), true)) && listed.contains(&(ours.get(), false)),
            "{listed:?}"
        );
    });
}

/// **Under `force`, a client asking to draw its own is told the
/// compositor draws it**, and framed.
#[test]
fn force_tells_a_client_asking_for_its_own_frame_that_solium_draws_it() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = framed_desk();
        desk.state.frames = policy(Policy::Force);
        desk.client.decoration_modes.clear();
        let pane = open(&mut desk, Says::Mode(Asks::ClientSide), "app");
        assert_eq!(framed(&desk, pane), Some(Whole));
        assert_eq!(
            desk.client.decoration_modes.last(),
            Some(&Asks::ServerSide),
            "told {:?}",
            desk.client.decoration_modes
        );
    });
}

/// **Under `allow`, a client that leaves it to the compositor is
/// offered its own frame.**
#[test]
fn allow_offers_a_client_that_leaves_it_to_us_its_own_frame() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = framed_desk();
        desk.state.frames = policy(Policy::Allow);
        desk.client.decoration_modes.clear();
        let pane = open(&mut desk, Says::Unset, "app");
        assert_eq!(framed(&desk, pane), Some(ClientSide));
        assert_eq!(
            desk.client.decoration_modes.last(),
            Some(&Asks::ClientSide),
            "told {:?}",
            desk.client.decoration_modes
        );
    });
}

/// **A rule names an application by `app_id`**, the last that
/// matches winning over the policy, for a client that never asks
/// and one that does.
#[test]
fn a_rule_gives_the_applications_it_names_their_own_policy() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = framed_desk();
        desk.state.frames = Settings {
            policy: Policy::Prefer,
            rules: vec![
                Rule {
                    app_id: crate::effect::rules::Glob::parse("org.gnome.*"),
                    policy: Policy::Force,
                },
                Rule {
                    app_id: crate::effect::rules::Glob::parse("org.gnome.Nautilus"),
                    policy: Policy::Allow,
                },
            ],
        };
        let calculator = open(&mut desk, Says::Nothing, "org.gnome.Calculator");
        let files = open(&mut desk, Says::Unset, "org.gnome.Nautilus");
        let other = open(&mut desk, Says::Nothing, "firefox");
        assert_eq!(
            [calculator, files, other].map(|pane| framed(&desk, pane)),
            [Some(Whole), Some(ClientSide), Some(ClientSide)],
            "(a gnome app forced, Nautilus allowed, firefox preferred)"
        );
    });
}

/// **A client naming itself after its first frame is decided
/// again** -- GTK sets its `app_id` late -- so a rule for it holds.
#[test]
fn a_window_is_decided_again_when_its_client_names_itself() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = framed_desk();
        desk.state.frames = Settings {
            policy: Policy::Prefer,
            rules: vec![Rule {
                app_id: crate::effect::rules::Glob::parse("late"),
                policy: Policy::Force,
            }],
        };
        let pane = open(&mut desk, Says::Nothing, "early");
        assert_eq!(
            framed(&desk, pane),
            Some(ClientSide),
            "the premise: client-side as `early`"
        );
        let toplevel = desk
            .state
            .panes
            .get(pane)
            .and_then(Pane::client)
            .and_then(Window::toplevel)
            .cloned()
            .expect("an xdg toplevel");
        // The server's view of the rename, as `app_id_changed` hears it.
        with_states(toplevel.wl_surface(), |states| {
            if let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>()
                && let Ok(mut attributes) = data.lock()
            {
                attributes.app_id = Some("late".to_owned());
            }
        });
        XdgShellHandler::app_id_changed(&mut desk.state, toplevel);
        desk.pump();
        assert_eq!(framed(&desk, pane), Some(Whole));
    });
}

/// **A change of policy reframes the windows already open**, and
/// back.
#[test]
fn a_change_of_policy_reframes_the_open_windows() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = framed_desk();
        let never = open(&mut desk, Says::Nothing, "gtk");
        let own = open(&mut desk, Says::Mode(Asks::ClientSide), "electron");
        let ours = open(&mut desk, Says::Unset, "qt");
        let seen = |desk: &Desk| [never, own, ours].map(|pane| framed(desk, pane));
        assert_eq!(
            seen(&desk),
            [Some(ClientSide), Some(ClientSide), Some(Whole)]
        );
        frames(&mut desk, policy(Policy::Force));
        desk.pump();
        assert_eq!(seen(&desk), [Some(Whole); 3], "forced");
        frames(&mut desk, policy(Policy::Allow));
        desk.pump();
        assert_eq!(seen(&desk), [Some(ClientSide); 3], "allowed");
        frames(&mut desk, Settings::default());
        desk.pump();
        assert_eq!(
            seen(&desk),
            [Some(ClientSide), Some(ClientSide), Some(Whole)],
            "back"
        );
    });
}

/// **A GTK3 client asks through KDE's protocol and is framed as
/// it asks** (#61): server-side, the whole frame; client-side, as
/// Firefox asks for its tabs, the client-side one -- and under
/// `force` it is told server-side, through the object it made.
#[test]
fn a_gtk3_client_is_framed_as_it_asks_through_kdes_protocol() {
    // Frames are Qt scenes, and Qt lives on one thread.
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        use org_kde_kwin_server_decoration::Mode as Kde;
        let mut desk = framed_desk();
        let whole = open(&mut desk, Says::Kde(Kde::Server), "gtk3");
        let own = open(&mut desk, Says::Kde(Kde::Client), "firefox");
        assert_eq!(
            (framed(&desk, whole), framed(&desk, own)),
            (Some(Whole), Some(ClientSide))
        );
        let mut desk = framed_desk();
        desk.state.frames = policy(Policy::Force);
        desk.client.kde_modes.clear();
        let forced = open(&mut desk, Says::Kde(Kde::Client), "firefox");
        assert_eq!(framed(&desk, forced), Some(Whole));
        assert_eq!(
            desk.client.kde_modes.last(),
            Some(&Kde::Server),
            "told {:?}",
            desk.client.kde_modes
        );
    });
}
