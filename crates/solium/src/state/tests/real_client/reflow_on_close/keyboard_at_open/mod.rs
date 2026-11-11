//! **#134 review: a window opened onto a workspace nobody is looking
//! at took the keyboard with it.**
//!
//! Inside `reflow_on_close` for [`Desk`], the one fixture with the
//! shipped scripts and a real client, and this needs both: the
//! scripts to decide where a window goes, and the client to say
//! where a key lands.
//!
//! With `follow_overflow = false` a window with no room is placed on
//! the next empty workspace, a screen away, and the view stays.
//! `new_toplevel` gave every new toplevel the keyboard -- before any
//! layout had said where it goes, or, for a window launched with
//! `sol.spawn`, after the layout had already put it there -- and
//! `scrolling.lua`, while not in charge, focused it again. Every
//! key typed after that went to a window the user could not see,
//! until they clicked. That is #127's fault by another door.
//!
//! **The second round: every way a new window gets the keyboard.**
//! Closing those two doors closed a third that nobody knew was one:
//! the strip's stray focus was the only way an X11 window ever got
//! the keyboard as it opened. And a fourth stayed open: a launched
//! application activating with the token `sol.spawn` gave it, which
//! the first round's client never sent. So the client here sends it,
//! and each way in is pinned in each mode it can happen in.

use super::*;

/// The shipped layouts, as `init.lua` requires them, at a
/// minimum one window fills: the desk's one tile at the shipped
/// gap is 1896x1056, and neither half of it -- 942 across, 522
/// down -- is 1000x600. With [`SHIPPED_HEARING_FOCUS`]'s
/// recorder, which is those same layouts.
fn one_window_fills_a_workspace(follow: bool) -> String {
    format!(
        "local config = require(\"config\")\n\
         config.tiling.minimum = {{ w = 1000, h = 600 }}\n\
         config.tiling.follow_overflow = {follow}\n\
         {SHIPPED_HEARING_FOCUS}"
    )
}

/// Tiling in charge and one window open, with the keyboard,
/// typing reaching it, and every animation landed.
fn working_in_one(follow: bool) -> (Desk, Window) {
    working_in(&one_window_fills_a_workspace(follow), Some("super+t"))
}

/// [`working_in_one`] for any scripts, in the mode `key`
/// switches on -- floating, the startup default, for `None`.
fn working_in(scripts: &str, key: Option<&str>) -> (Desk, Window) {
    let mut desk = Desk::new();
    desk.install(scripts);
    if let Some(key) = key {
        assert!(desk.state.trigger(key), "{key} was not handled");
    }
    let (working, _, _) = desk.open();
    // Twice more, for the reason `typing_after_a_close...`
    // gives: the keyboard is a request the client makes in
    // answer to the seat's capabilities.
    desk.pump();
    desk.pump();
    assert!(
        desk.client.keyboard.is_some(),
        "the client bound a keyboard; without one this test cannot \
         observe anything"
    );
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "the premise: the window the user is working in has the keyboard"
    );
    typed_into(&mut desk, &working, "the premise: typing reaches it");
    (desk, working)
}

/// One key, and where it landed, as the client saw it.
fn typed_into(desk: &mut Desk, window: &Window, says: &str) {
    types(&mut desk.state, KEY_A);
    desk.pump();
    assert_eq!(
        desk.client.typed.last(),
        Some(&(Some(surface_id(window)), KEY_A)),
        "{says}"
    );
}

/// Whether a pane is headed somewhere the user can see it, as
/// the focus rules ask.
fn headed_on_stage(state: &Solium, pane: crate::pane::PaneId) -> bool {
    state.pane_on_stage(pane)
}

/// What the scripts say, for a premise.
fn says(desk: &Desk, chunk: &str) -> String {
    desk.state
        .scripts
        .as_ref()
        .map(|scripts| scripts.evaluate(chunk))
        .unwrap_or_default()
}

fn workspace_of(desk: &Desk, pane: crate::pane::PaneId) -> String {
    says(
        desk,
        &format!(
            "return tostring(require(\"workspaces\").of[{}])",
            pane.get()
        ),
    )
}

fn showing(desk: &Desk) -> String {
    says(
        desk,
        "return tostring(require(\"workspaces\").on(\"reflow-test\"))",
    )
}

/// What `sol.spawn` does before it forks, for a program whose
/// process is this one -- so the client this fixture connects
/// is the application it asked for. `open_loading` rather than
/// `begin_loading`, which would build the loading scene in Qt:
/// see `Solium::open_loading`.
fn asked_for(desk: &mut Desk) -> crate::pane::PaneId {
    let source = crate::pane::loading_source(None);
    desk.state
        .open_loading("app", Some(std::process::id()), source, None)
}

/// The application asked for arrives: a toplevel and its first
/// frame, in one dispatch.
fn arrives(desk: &mut Desk) -> Window {
    let (window, _toplevel, _surface, _xdg) = open_xdg(
        &mut desk.display,
        &mut desk.state,
        &desk.conn,
        &desk.client,
        &desk.qh,
    );
    desk.state.sync_panes();
    desk.pump();
    window
}

/// **A window opened by its application, with nowhere on this
/// workspace to go and `follow_overflow` off, leaves the
/// keyboard where it was.**
#[test]
fn a_window_that_overflows_to_a_hidden_workspace_does_not_take_the_keyboard() {
    let (mut desk, working) = working_in_one(false);
    let (_parked, _, pane) = desk.open();
    assert_eq!(
        workspace_of(&desk, pane),
        "2",
        "the premise: the new window was sent to workspace 2"
    );
    assert_eq!(showing(&desk), "1", "the premise: the view stayed");
    assert!(
        !headed_on_stage(&desk.state, pane),
        "the premise: it is parked a screen away"
    );

    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "the keyboard went with the window to a workspace nobody is looking at"
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after the window opened went somewhere else",
    );
}

/// **The same for a window launched with `sol.spawn`**, whose
/// place is decided before its application exists: it is on
/// workspace 2 already when the application arrives, and the
/// application arriving is not a reason to take the keyboard
/// there.
#[test]
fn a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_when_it_arrives() {
    let (mut desk, working) = working_in_one(false);
    let pane = asked_for(&mut desk);
    assert_eq!(
        workspace_of(&desk, pane),
        "2",
        "the premise: the launched window was sent to workspace 2"
    );
    assert!(
        !headed_on_stage(&desk.state, pane),
        "the premise: it is parked a screen away"
    );

    let arrived = arrives(&mut desk);
    assert_eq!(
        desk.state.panes.id_of(&arrived),
        Some(pane),
        "the premise: the application arrived in the window opened for it"
    );
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "the application arriving took the keyboard to a workspace nobody is \
         looking at"
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after the application arrived went somewhere else",
    );
}

/// **With `follow_overflow` on, a launched window that
/// overflows takes the keyboard when its application arrives.**
///
/// The view goes to workspace 2 at the launch and `tiling.lua`
/// focuses the window by name, but `open` for a launched window
/// fires before its application exists and a window with no
/// client cannot be given the keyboard (`window_by_id`), so that
/// request does nothing. Until the application arrives the
/// keyboard is on nothing: the window it was on is a screen away
/// now, on a desk a selection has put away, and gives it up as
/// one sent there does (#403).
///
/// The half this pins is the arrival. The keyboard used to be
/// given in `new_toplevel` and is given at the window's first
/// frame now, when it is known to be on screen; that move is
/// what could have lost it.
#[test]
fn a_launched_window_that_overflows_with_the_view_takes_the_keyboard_when_it_arrives() {
    let (mut desk, working) = working_in_one(true);
    let pane = asked_for(&mut desk);
    assert_eq!(
        workspace_of(&desk, pane),
        "2",
        "the premise: the launched window was sent to workspace 2"
    );
    assert_eq!(showing(&desk), "2", "the premise: the view went with it");
    assert!(
        headed_on_stage(&desk.state, pane),
        "the premise: it is headed on screen"
    );
    let left = desk
        .state
        .panes
        .id_of(&working)
        .expect("an open window has a pane");
    assert!(
        !headed_on_stage(&desk.state, left),
        "the premise: the view has left the window that was working"
    );
    assert_eq!(
        desk.state.focused_window(),
        None,
        "until the application arrives the keyboard is on nothing, and not on \
         {working:?}, which the view has just left"
    );

    let arrived = arrives(&mut desk);
    assert_eq!(desk.state.panes.id_of(&arrived), Some(pane));
    assert_eq!(
        desk.state.focused_window(),
        Some(arrived.clone()),
        "the application arrived in a window on screen and was not given the \
         keyboard"
    );
    typed_into(
        &mut desk,
        &arrived,
        "a key typed after the application arrived did not reach it",
    );
}

/// **#403: a window sent to another workspace leaves the
/// keyboard on a window in view, and with none, on nothing.**
/// `super+shift+2` moved the focused window onto a desk a
/// selection carries a screen away and left the keyboard on it,
/// so what was typed next went into a window nobody could see.
/// The keyboard goes where `focus.after_close` sends it, as it
/// does when a window closes; here the one window left on screen.
#[test]
fn a_window_sent_to_another_workspace_leaves_the_keyboard_on_the_one_in_view() {
    let (mut desk, working) = working_in(SHIPPED_HEARING_FOCUS, Some("super+t"));
    let (sent, _, pane) = desk.open();
    assert_eq!(
        desk.state.focused_window(),
        Some(sent.clone()),
        "the premise: the window opened has the keyboard"
    );

    assert!(desk.state.trigger("super+shift+2"));
    desk.pump();
    assert_eq!(
        workspace_of(&desk, pane),
        "2",
        "the premise: the window was sent to workspace 2"
    );
    assert_eq!(showing(&desk), "1", "the premise: the view stayed");
    assert!(
        !headed_on_stage(&desk.state, pane),
        "the premise: it is parked a screen away"
    );
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "the keyboard stayed on the window sent away"
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after the send went somewhere else",
    );

    // The last window in view, sent too: nothing is left on screen
    // to type into, and nothing has the keyboard.
    assert!(desk.state.trigger("super+shift+2"));
    desk.pump();
    assert_eq!(
        desk.state.focused_window(),
        None,
        "the keyboard stayed on a window sent away with nothing left in view"
    );
}

/// **A script that moves the keyboard as a window opens has the
/// last word.** While `new_toplevel` gave the keyboard, it did so
/// before `open` and a script's `sol.focus` there came after it.
/// The keyboard is given after `open` now, and must not take a
/// script's choice back: this one keeps it on the window that was
/// already open.
#[test]
fn a_script_that_moves_the_keyboard_at_open_has_the_last_word() {
    let mut desk = Desk::new();
    desk.install(
        "sol.on(\"open\", function(id)\n\
             for _, window in ipairs(sol.windows()) do\n\
                 if window.id ~= id then\n\
                     sol.focus(window.id)\n\
                     return\n\
                 end\n\
             end\n\
         end)",
    );
    let (first, _, _) = desk.open();
    desk.pump();
    desk.pump();
    assert_eq!(
        desk.state.focused_window(),
        Some(first.clone()),
        "the premise: with nothing else open and no script moving it, the new \
         window has the keyboard"
    );
    let _second = desk.open();
    assert_eq!(
        desk.state.focused_window(),
        Some(first.clone()),
        "the script put the keyboard back on the first window and the compositor \
         took it away again"
    );
    typed_into(&mut desk, &first, "the key went to the new window");
}

/// The shipped layouts as `init.lua` requires them, with every
/// `focus` the scripts hear written down in `heard`.
const SHIPPED_HEARING_FOCUS: &str = "require(\"modes\")\n\
     require(\"workspaces\")\n\
     require(\"tiling\")\n\
     require(\"scrolling\")\n\
     heard = {}\n\
     sol.on(\"focus\", function(id) heard[#heard + 1] = id end)";

/// The `focus` events the scripts have heard, oldest first.
fn heard(desk: &Desk) -> String {
    says(desk, "return table.concat(heard, \",\")")
}

/// The window in a pane.
fn window_of(desk: &Desk, pane: crate::pane::PaneId) -> Window {
    desk.state
        .panes
        .get(pane)
        .and_then(Pane::client)
        .cloned()
        .expect("the pane has its client")
}

/// A second window opens in the mode `key` switches on, and
/// takes the keyboard as a focus: typing reaches it, and the
/// scripts hear `focus` for it. A bare grant gives the first two
/// and not the third.
fn opening_takes_the_keyboard_as_a_focus(key: Option<&str>, mode: &str) {
    let (mut desk, _working) = working_in(SHIPPED_HEARING_FOCUS, key);
    let (arrived, _, pane) = desk.open();
    assert_eq!(
        desk.state.focused_window(),
        Some(arrived.clone()),
        "{mode}: the new window was not given the keyboard"
    );
    typed_into(
        &mut desk,
        &arrived,
        &format!("{mode}: a key typed after the window opened went somewhere else"),
    );
    let heard = heard(&desk);
    assert_eq!(
        heard.rsplit(',').next(),
        Some(pane.get().to_string().as_str()),
        "{mode}: the new window took the keyboard and the scripts never heard \
         `focus` for it -- a bare grant rather than a focus. Heard: [{heard}]"
    );
}

/// **Floating, the startup default.** Nothing in the scripts
/// focuses a new window here, so this is `offer_keyboard`'s own
/// path. Before #134's second review it was a bare
/// `give_keyboard`: the keyboard, and no `focus`.
#[test]
fn a_window_opening_while_floating_takes_the_keyboard_as_a_focus() {
    opening_takes_the_keyboard_as_a_focus(None, "floating");
}

/// **Tiling**, which focuses a new window only when the view
/// goes with it to another workspace -- otherwise the same path
/// as floating, and the same bare grant before.
#[test]
fn a_window_opening_while_tiling_takes_the_keyboard_as_a_focus() {
    opening_takes_the_keyboard_as_a_focus(Some("super+t"), "tiling");
}

/// **Scrolling**, whose strip focuses its new column itself, so
/// `offer_keyboard` stands aside (`Opened::focused`). This held
/// before as well; it is here so that the three modes are held
/// to the same answer.
#[test]
fn a_window_opening_while_scrolling_takes_the_keyboard_as_a_focus() {
    opening_takes_the_keyboard_as_a_focus(Some("super+s"), "scrolling");
}

/// **With `follow_overflow` on, a window its application opened
/// with no room here takes the keyboard on the workspace the view
/// went to.** `tiling.lua` focuses it by name; this is the half
/// of `follow_overflow` its launched twin,
/// `a_launched_window_that_overflows_with_the_view_takes_the_keyboard_when_it_arrives`,
/// does not cover.
#[test]
fn a_window_that_overflows_with_the_view_takes_the_keyboard() {
    let (mut desk, _working) = working_in_one(true);
    let (arrived, _, pane) = desk.open();
    assert_eq!(
        workspace_of(&desk, pane),
        "2",
        "the premise: the new window was sent to workspace 2"
    );
    assert_eq!(showing(&desk), "2", "the premise: the view went with it");
    assert_eq!(
        desk.state.focused_window(),
        Some(arrived.clone()),
        "the view went with the new window and the keyboard did not"
    );
    typed_into(
        &mut desk,
        &arrived,
        "a key typed after the window opened did not reach it",
    );
}

/// **A managed X11 window reaching `offer_keyboard` is focused
/// exactly as an xdg one is.**
///
/// At the rule, because an `X11Surface` needs a live XWayland and
/// this binary cannot start one. What the rule's `Focus` leads to
/// is one line of `offer_keyboard` for both kinds,
/// `focus_window`, and the xdg tests above are what see that line
/// taken: a bare grant would give the keyboard without the
/// `focus` they listen for. Before #134's second review
/// `offer_keyboard` returned early for any window without an
/// `xdg_toplevel` -- `ClientKind::X11 => Stay`, in this rule's
/// terms -- and nothing else gave an X11 window the keyboard as
/// it opened, once `scrolling.lua` stopped doing it by accident.
/// That an XWayland application really comes up focused, and
/// draws itself so, is for a person to check by hand.
///
/// **And the two halves the rule does not see** (#134's third
/// review): which kind a window with only an X11 role is, through
/// the same [`ClientKind::from_roles`] that `ClientKind::of`
/// answers with; and that `offer_keyboard` has no early return
/// of its own. The second is asked of the source text, for the
/// reason `render.rs`'s drag-icon test gives: the behaviour
/// needs an X11 window, and restoring the old `return` passed
/// every other test. It pins only that `offer_keyboard` hands
/// the window's kind straight to this rule and returns nowhere
/// on the way.
#[test]
fn a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is() {
    assert_eq!(
        [
            ClientKind::from_roles(true, false),
            ClientKind::from_roles(false, true),
            ClientKind::from_roles(false, false),
        ],
        [Some(ClientKind::Xdg), Some(ClientKind::X11), None],
        "a window's roles name the wrong kind: an X11 window is one with only an \
         X11 surface"
    );
    for kind in [ClientKind::Xdg, ClientKind::X11].map(Some) {
        assert_eq!(
            first_focus(kind, true, true),
            FirstFocus::Focus,
            "{kind:?}: a managed window headed on screen was not focused"
        );
        assert_eq!(
            first_focus(kind, true, false),
            FirstFocus::Stay,
            "{kind:?}: a window headed where nobody can see it took the keyboard"
        );
        assert_eq!(
            first_focus(kind, false, true),
            FirstFocus::Stay,
            "{kind:?}: a menu, a tooltip or a splash took the keyboard"
        );
    }
    assert_eq!(
        first_focus(None, true, true),
        FirstFocus::Stay,
        "a window of no kind anybody knows took the keyboard"
    );

    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/state/open.rs"));
    let start = source
        .find("    fn offer_keyboard(&mut self, window: &Window, pane: crate::pane::PaneId) {")
        .expect("`offer_keyboard` is still here");
    let body = source[start..]
        .find("\n    }\n")
        .map(|end| &source[start..start + end])
        .expect("and still ends");
    assert!(
        body.contains("first_focus(")
            && body.contains("ClientKind::of(window)")
            && !body.contains("return")
            && !body.contains("toplevel()"),
        "`offer_keyboard` decides something before `first_focus` does -- an early \
         return, or a question about the toplevel -- so an X11 window can be \
         refused where no test can see it:\n{body}"
    );
}

/// When an application sends its activation token, against its
/// first frame.
#[derive(Clone, Copy, Debug)]
enum Sends {
    /// With the toplevel, before it has drawn: what winit does,
    /// and so alacritty.
    BeforeItsFirstFrame,
    /// Once it has drawn.
    AfterItsFirstFrame,
}

const BOTH: [Sends; 2] = [Sends::BeforeItsFirstFrame, Sends::AfterItsFirstFrame];

/// Let a frame go by: the panes reconciled, and a round trip.
fn frame(desk: &mut Desk) {
    desk.state.sync_panes();
    desk.pump();
}

/// A window arrives, and asks to be brought forward with
/// `token` when `sends` says -- as an application launched with
/// one does as it opens.
fn arrives_activating(desk: &mut Desk, token: &str, sends: Sends) -> Window {
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
    let activation = desk
        .client
        .activation
        .clone()
        .expect("xdg_activation_v1 bound");
    let before: Vec<Window> = desk.state.space.elements().cloned().collect();
    let surface = compositor.create_surface(&desk.qh, ());
    let xdg = wm_base.get_xdg_surface(&surface, &desk.qh, ());
    let _toplevel = xdg.get_toplevel(&desk.qh, ());
    desk.client.leave_frame(&_toplevel, &desk.qh);
    if matches!(sends, Sends::BeforeItsFirstFrame) {
        activation.activate(token.to_owned(), &surface);
    }
    commit_buffer(&desk.client, &desk.qh, &surface, 64, 64);
    desk.pump();
    let window = desk
        .state
        .space
        .elements()
        .find(|window| !before.contains(window))
        .cloned()
        .expect("new_toplevel mapped a window");
    frame(desk);
    if matches!(sends, Sends::AfterItsFirstFrame) {
        activation.activate(token.to_owned(), &surface);
        frame(desk);
    }
    window
}

/// A token nothing was launched with: what a notification, or
/// another application, hands a window it wants brought forward.
fn genuine_token(desk: &mut Desk) -> String {
    let (token, _) = desk
        .state
        .activation_state
        .create_external_token(XdgActivationTokenData::default());
    token.as_str().to_owned()
}

/// `surface` asks to be brought forward with `token`.
fn activates(desk: &mut Desk, surface: &wl_surface::WlSurface, token: &str) {
    desk.client
        .activation
        .clone()
        .expect("xdg_activation_v1 bound")
        .activate(token.to_owned(), surface);
    frame(desk);
}

/// **A launched window parked on a hidden workspace does not take
/// the keyboard by its own token**, sent before its first frame
/// or after.
///
/// The application is the process `sol.spawn` started, so
/// `new_toplevel` adopts it by its pid and its window is no
/// longer loading by the time the token comes back.
/// `claim_into` asked "still loading?" first, said no, and
/// `request_activation` took the launch's own token for an
/// ordinary request and focused the window, on a desk nobody is
/// looking at. `a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_when_it_arrives`
/// could not see it: its client sends no token.
#[test]
fn a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_by_its_own_token() {
    for sends in BOTH {
        let (mut desk, working) = working_in_one(false);
        let pane = asked_for(&mut desk);
        let token = desk.state.launch_token(pane);
        assert!(
            !headed_on_stage(&desk.state, pane),
            "{sends:?}: the premise: it is parked a screen away"
        );
        let arrived = arrives_activating(&mut desk, &token, sends);
        assert_eq!(
            desk.state.panes.id_of(&arrived),
            Some(pane),
            "{sends:?}: the premise: the application arrived in the window \
             opened for it"
        );
        assert_eq!(
            desk.state.focused_window(),
            Some(working.clone()),
            "{sends:?}: the launch's own token took the keyboard to a workspace \
             nobody is looking at"
        );
        typed_into(
            &mut desk,
            &working,
            &format!("{sends:?}: a key typed after the token went somewhere else"),
        );
        // Not even for a moment. Focused and then handed back,
        // the window would have been told it had the keyboard
        // and the clipboard, and the scripts that it was
        // focused, over a token that asks for none of it.
        let (heard, parked) = (heard(&desk), pane.get().to_string());
        assert!(
            !heard.split(',').any(|id| id == parked),
            "{sends:?}: the launch's own token focused the parked window, if only \
             until something took the keyboard back. Heard: [{heard}]"
        );
    }
}

/// **The other half: a launched window on screen that activates
/// with its own token has the keyboard**, whichever side of its
/// first frame the token comes. The token decides nothing about
/// the keyboard any more; `offer_keyboard` does, at the first
/// frame, and this is it saying yes.
#[test]
fn a_launched_window_on_screen_takes_the_keyboard_when_it_activates_with_its_own_token() {
    for sends in BOTH {
        let (mut desk, _working) = working_in_one(true);
        let pane = asked_for(&mut desk);
        let token = desk.state.launch_token(pane);
        assert!(
            headed_on_stage(&desk.state, pane),
            "{sends:?}: the premise: the view went with it"
        );
        let arrived = arrives_activating(&mut desk, &token, sends);
        assert_eq!(desk.state.panes.id_of(&arrived), Some(pane));
        assert_eq!(
            desk.state.focused_window(),
            Some(arrived.clone()),
            "{sends:?}: the application arrived on screen and was not given the \
             keyboard"
        );
        let heard = heard(&desk);
        assert_eq!(
            heard.rsplit(',').next(),
            Some(pane.get().to_string().as_str()),
            "{sends:?}: the application took the keyboard and the scripts never \
             heard `focus` for it. Heard: [{heard}]"
        );
        typed_into(
            &mut desk,
            &arrived,
            &format!("{sends:?}: a key typed after it arrived did not reach it"),
        );
    }
}

/// **A genuine activation of a window on screen takes the
/// keyboard**, as the protocol is for: the check that follows it
/// in `request_activation` leaves it alone.
#[test]
fn a_genuine_activation_of_a_window_on_screen_takes_the_keyboard() {
    let (mut desk, _working) = working_in(SHIPPED_HEARING_FOCUS, None);
    let asking = desk.open_surface();
    let (other, _, _) = desk.open();
    assert_eq!(
        desk.state.focused_window(),
        Some(other),
        "the premise: the window opened last has the keyboard"
    );
    let token = genuine_token(&mut desk);
    activates(&mut desk, &asking.surface, &token);
    let asking = window_of(&desk, asking.pane);
    assert_eq!(
        desk.state.focused_window(),
        Some(asking.clone()),
        "a window on screen asked to be brought forward and was refused"
    );
    typed_into(&mut desk, &asking, "a key typed after it did not reach it");
}

/// **A genuine activation of a window being closed does not
/// keep the keyboard.** Its own frame is fading to nothing, and
/// no selection is what keeps it off stage. Until #134's fifth
/// review it was focused like any other window and then, headed
/// nowhere the user can see, gave the keyboard back to what is
/// on screen; it is refused before it is focused now, which
/// with one other window on screen ends in the same place --
/// `a_genuine_activation_of_a_window_being_closed_on_the_desk_in_view_leaves_the_keyboard_exactly_where_it_was`
/// is the one that tells the two apart.
#[test]
fn a_genuine_activation_of_a_window_being_closed_does_not_keep_the_keyboard() {
    let (mut desk, working) = working_in(SHIPPED_HEARING_FOCUS, None);
    let closing = desk.open_surface();
    desk.state.focus_window(
        &working,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    typed_into(
        &mut desk,
        &working,
        "the premise: typing reaches the window before",
    );
    desk.state.close_pane(closing.pane);
    assert!(
        !headed_on_stage(&desk.state, closing.pane)
            && !desk.state.carried_by_a_selection(closing.pane),
        "the premise: the window being closed is headed off stage, and not by a \
         selection"
    );

    let token = genuine_token(&mut desk);
    activates(&mut desk, &closing.surface, &token);
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "a window being closed asked to be brought forward and kept the keyboard"
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after it went somewhere else",
    );
}

/// **A genuine activation of a window its own frame hides on the
/// desk in view is focused, and does not keep the keyboard.**
/// No selection hides it and it is not being closed, so nothing
/// refuses it first: focusing a window can bring it back, as
/// the strip brings back a column
/// (`a_genuine_activation_of_a_column_scrolled_off_screen_brings_it_back_with_the_keyboard`).
/// The check after the focus in `request_activation` is what
/// gives the keyboard back to what is on screen when nothing
/// did.
#[test]
fn a_genuine_activation_of_a_window_its_own_frame_hides_does_not_keep_the_keyboard() {
    let (mut desk, working) = working_in(SHIPPED_HEARING_FOCUS, None);
    let hidden = desk.open_surface();
    desk.state.focus_window(
        &working,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    typed_into(
        &mut desk,
        &working,
        "the premise: typing reaches the window before",
    );
    // Held at nothing where it is, as `sol.present(id, { opacity
    // = 0 })` holds a window.
    let now = desk.state.clock.now();
    if let Some(pane) = desk.state.panes.get(hidden.pane) {
        let outer = desk.state.pane_outer(pane);
        present::present(
            pane,
            outer,
            Frame {
                opacity: 0.0,
                ..Frame::real(outer)
            },
            now,
            Duration::ZERO,
            solium_animation::Curve::Linear,
            None,
        );
    }
    assert!(
        !headed_on_stage(&desk.state, hidden.pane)
            && !desk.state.carried_by_a_selection(hidden.pane)
            && !desk.state.panes.get(hidden.pane).is_some_and(Pane::leaving),
        "the premise: the window is held invisible by its own frame, and by \\
             nothing else"
    );

    let before = heard(&desk);
    let token = genuine_token(&mut desk);
    activates(&mut desk, &hidden.surface, &token);
    let after = heard(&desk);
    let since = after.get(before.len()..).unwrap_or_default();
    assert!(
        since
            .split(',')
            .any(|id| id == hidden.pane.get().to_string()),
        "the premise: it was focused, which is what the check after the focus is \\
             for. Heard since: [{since}]"
    );
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "a window its own frame hides asked to be brought forward and kept the \\
             keyboard"
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after it went somewhere else",
    );
    // Turned down because nobody can see it, as a window on a
    // hidden desk is, so it wants you just the same (Ruling 18).
    assert!(
        desk.state.urgent.contains(&hidden.pane.get()),
        "a window nobody can see asked to be brought forward, gave the keyboard \
         back, and is not urgent"
    );
}

/// **Any client can mint itself a token, and take the keyboard
/// with it.** Pinned as it stands, not as it ought to be: this
/// compositor does not override `token_created`, smithay's
/// default keeps every token a client asks for -- with no input
/// serial and no surface, as here -- and `request_activation`
/// honours whatever token comes back. There is no focus-stealing
/// prevention yet, and the comment there said a client could not
/// do this until #134's third review. When there is, this test
/// is the one that has to change.
#[test]
fn a_client_can_mint_itself_an_activation_token_and_take_the_keyboard_with_it() {
    let (mut desk, working) = working_in(SHIPPED_HEARING_FOCUS, None);
    let asking = desk.open_surface();
    desk.state.focus_window(
        &working,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    typed_into(
        &mut desk,
        &working,
        "the premise: typing reaches the window before",
    );

    let activation = desk
        .client
        .activation
        .clone()
        .expect("xdg_activation_v1 bound");
    // Nothing set on it: no serial from any input event, and no
    // surface asking.
    activation.get_activation_token(&desk.qh, ()).commit();
    frame(&mut desk);
    let token = desk
        .client
        .tokens
        .last()
        .cloned()
        .expect("the compositor handed the client the token it asked for");
    activates(&mut desk, &asking.surface, &token);

    let asking = window_of(&desk, asking.pane);
    assert_eq!(
        desk.state.focused_window(),
        Some(asking.clone()),
        "a token a client minted for itself was refused: focus-stealing \
         prevention exists now, and `request_activation`'s comment and this test \
         both say otherwise"
    );
    typed_into(&mut desk, &asking, "a key typed after it did not reach it");
}

/// **A genuine activation of a window on a workspace nobody is
/// looking at leaves the keyboard on screen.** Before #134's
/// second review `focus_window` gave it the keyboard there and
/// nothing took it back; with `follow_overflow = false` that
/// window is an ordinary one.
#[test]
fn a_genuine_activation_of_a_window_on_a_hidden_workspace_leaves_the_keyboard_on_screen() {
    let (mut desk, working) = working_in_one(false);
    let parked = desk.open_surface();
    assert_eq!(
        workspace_of(&desk, parked.pane),
        "2",
        "the premise: the new window was sent to workspace 2"
    );
    assert!(
        !headed_on_stage(&desk.state, parked.pane),
        "the premise: it is parked a screen away"
    );
    let token = genuine_token(&mut desk);
    activates(&mut desk, &parked.surface, &token);
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "an activation took the keyboard to a workspace nobody is looking at"
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after the activation went somewhere else",
    );
}

/// **A genuine activation of a column scrolled off the screen
/// brings it back, keyboard and all** -- which is why
/// `request_activation` asks whether the window is on screen
/// after focusing it and not before: the strip scrolls a focused
/// column into view, so the column is off screen only until the
/// focus it asked for. Asking first refused it.
#[test]
fn a_genuine_activation_of_a_column_scrolled_off_screen_brings_it_back_with_the_keyboard() {
    let (mut desk, _working) = working_in(SHIPPED_HEARING_FOCUS, Some("super+s"));
    let first = desk.open_surface();
    for _ in 0..4 {
        let _ = desk.open();
    }
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    assert!(
        !headed_on_stage(&desk.state, first.pane),
        "the premise: the strip scrolled the column off screen"
    );
    let token = genuine_token(&mut desk);
    activates(&mut desk, &first.surface, &token);
    let first_window = window_of(&desk, first.pane);
    assert_eq!(
        desk.state.focused_window(),
        Some(first_window.clone()),
        "a column off the edge asked to be brought forward and was refused"
    );
    assert!(
        headed_on_stage(&desk.state, first.pane),
        "the strip did not bring the column it focused back into view"
    );
    typed_into(
        &mut desk,
        &first_window,
        "a key typed after the activation did not reach the column",
    );
}

/// Scrolling, one monitor: five windows on workspace 1, so the
/// strip has scrolled `off` -- the second of them -- off the
/// screen; then workspace 2, with two columns side by side on it,
/// the keyboard on `left` and the pointer resting on `right`.
/// `shut` is another of workspace 1's. Every animation landed.
///
/// The pointer is on the other tile so that `settle_focus` --
/// which a window focused and handed back ends in -- picks a
/// window other than the one that had the keyboard.
struct LeftBehind {
    desk: Desk,
    off: Opened,
    shut: Opened,
    left: Window,
}

fn a_strip_left_behind_on_workspace_1() -> LeftBehind {
    let (mut desk, _working) = working_in(SHIPPED_HEARING_FOCUS, Some("super+s"));
    let off = desk.open_surface();
    let shut = desk.open_surface();
    for _ in 0..3 {
        let _ = desk.open();
    }
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    assert!(
        !headed_on_stage(&desk.state, off.pane),
        "the premise: the strip scrolled the column off screen"
    );

    assert!(desk.state.trigger("super+2"), "super+2 was not handled");
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);
    let left = desk.open_surface();
    let right = desk.open_surface();
    desk.answer(&left);
    desk.answer(&right);
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);
    assert_eq!(
        (
            showing(&desk),
            workspace_of(&desk, off.pane),
            workspace_of(&desk, shut.pane),
            workspace_of(&desk, left.pane),
            workspace_of(&desk, right.pane),
        ),
        (
            "2".to_owned(),
            "1".to_owned(),
            "1".to_owned(),
            "2".to_owned(),
            "2".to_owned()
        ),
        "the premise: the view is on workspace 2, `off` and `shut` on 1, the two \
         columns on 2"
    );
    assert!(
        headed_on_stage(&desk.state, left.pane)
            && headed_on_stage(&desk.state, right.pane)
            && !headed_on_stage(&desk.state, off.pane),
        "the premise: both of workspace 2's columns are on screen, and `off` is not"
    );

    let over_right = desk.placed(right.pane);
    point_at(
        &mut desk.state,
        (
            f64::from(over_right.loc.x + over_right.size.w / 2),
            f64::from(over_right.loc.y + over_right.size.h / 2),
        ),
    );
    let left = window_of(&desk, left.pane);
    desk.state.focus_window(
        &left,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    typed_into(&mut desk, &left, "the premise: typing reaches `left`");
    LeftBehind {
        desk,
        off,
        shut,
        left,
    }
}

/// The keyboard is on `left`, typing reaches it, and since
/// `before` the scripts have heard no focus for the window that
/// asked, `asked`.
fn still_on_the_left(desk: &mut Desk, left: &Window, before: &str, asked: u64, route: &str) {
    assert_eq!(
        desk.state.focused_window(),
        Some(left.clone()),
        "{route}: the keyboard moved off the window that had it"
    );
    typed_into(
        desk,
        left,
        &format!("{route}: a key typed afterwards went somewhere else"),
    );
    let after = heard(desk);
    let since = after.get(before.len()..).unwrap_or_default();
    let asked = asked.to_string();
    assert!(
        !since.split(',').any(|id| id == asked),
        "{route}: and the window that asked was focused on the way, if only until \
         the keyboard was handed on. Heard since: [{since}]"
    );
}

/// **#134's fourth review, finding 2: a column scrolled off a
/// hidden workspace is not focused by a genuine activation.**
/// Its own frame is off stage already, so asking whether a
/// selection carries that frame off -- as `carried_off_stage`
/// did -- said no; it was focused, the strip in view does not
/// hold it and brought nothing back, and `hand_off_keyboard`
/// gave the keyboard to whatever `settle_focus` picked: the
/// column under the pointer.
#[test]
fn a_genuine_activation_of_a_column_scrolled_off_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
 {
    let LeftBehind {
        mut desk,
        off,
        left,
        ..
    } = a_strip_left_behind_on_workspace_1();
    let before = heard(&desk);
    let token = genuine_token(&mut desk);
    activates(&mut desk, &off.surface, &token);
    still_on_the_left(
        &mut desk,
        &left,
        &before,
        off.pane.get(),
        "a column scrolled off workspace 1 asked to be brought forward",
    );
}

/// **The same for a window being closed on a hidden workspace**,
/// whose own frame is fading to nothing and so was off stage
/// before any selection was asked about it either.
#[test]
fn a_genuine_activation_of_a_window_being_closed_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
 {
    let LeftBehind {
        mut desk,
        shut,
        left,
        ..
    } = a_strip_left_behind_on_workspace_1();
    desk.state.close_pane(shut.pane);
    assert!(
        desk.state.panes.get(shut.pane).is_some_and(Pane::leaving),
        "the premise: the window on workspace 1 is being closed"
    );
    typed_into(
        &mut desk,
        &left,
        "the premise: the close left the keyboard on `left`",
    );
    let before = heard(&desk);
    let token = genuine_token(&mut desk);
    activates(&mut desk, &shut.surface, &token);
    still_on_the_left(
        &mut desk,
        &left,
        &before,
        shut.pane.get(),
        "a window being closed on workspace 1 asked to be brought forward",
    );
}

/// Tiling, one monitor, the desk in view: `left` with the
/// keyboard, and `right` and `third` tiled beside it, every
/// animation landed. See [`keyboard_left_pointer_right`] for
/// where the pointer goes.
struct ThreeTiles {
    desk: Desk,
    left: Window,
    right: Opened,
    third: Opened,
}

fn three_tiles() -> ThreeTiles {
    let (mut desk, left) = working_in(SHIPPED_HEARING_FOCUS, Some("super+t"));
    let right = desk.open_surface();
    let third = desk.open_surface();
    desk.answer(&right);
    desk.answer(&third);
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);
    let left_pane = desk
        .state
        .panes
        .id_of(&left)
        .expect("a client in the space has a pane");
    assert!(
        [left_pane, right.pane, third.pane]
            .iter()
            .all(|pane| headed_on_stage(&desk.state, *pane)
                && !desk.state.carried_by_a_selection(*pane)),
        "the premise: three tiles on the desk in view"
    );
    desk.state.focus_window(
        &left,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    ThreeTiles {
        desk,
        left,
        right,
        third,
    }
}

/// The pointer comes to rest on `right`'s tile as the layout
/// has it now, and the keyboard goes back to `left`.
///
/// On the other tile so that `settle_focus` -- which a window
/// focused and handed back ends in -- picks a window other than
/// the one that had the keyboard: asserted, since that is the
/// whole of what tells "handed off" from "stayed put" here.
fn keyboard_left_pointer_right(tiles: &mut ThreeTiles) {
    let ThreeTiles {
        desk, left, right, ..
    } = tiles;
    let over_right = desk.placed(right.pane);
    let at = Point::<f64, Logical>::from((
        f64::from(over_right.loc.x + over_right.size.w / 2),
        f64::from(over_right.loc.y + over_right.size.h / 2),
    ));
    point_at(&mut desk.state, (at.x, at.y));
    desk.state.focus_window(
        left,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    typed_into(desk, left, "the premise: typing reaches `left`");
    let right = window_of(desk, right.pane);
    assert_eq!(
        desk.state
            .window_under_at(at, desk.state.settling())
            .map(|(window, _)| window),
        Some(right),
        "the premise: `settle_focus`'s pointer arm would pick `right`"
    );
}

/// **#134's fifth review, finding 1: a genuine activation of a
/// window being closed on the desk in view leaves the keyboard
/// exactly where it was.** Refused before it is focused, as one
/// on a hidden desk is. Until then it was focused, found to be
/// headed nowhere, and handed off -- and `settle_focus` gave the
/// keyboard to the tile under the pointer, which is not the one
/// that had it.
#[test]
fn a_genuine_activation_of_a_window_being_closed_on_the_desk_in_view_leaves_the_keyboard_exactly_where_it_was()
 {
    let mut tiles = three_tiles();
    tiles.desk.state.close_pane(tiles.third.pane);
    assert!(
        tiles
            .desk
            .state
            .panes
            .get(tiles.third.pane)
            .is_some_and(Pane::leaving)
            && !headed_on_stage(&tiles.desk.state, tiles.third.pane)
            && !tiles.desk.state.carried_by_a_selection(tiles.third.pane),
        "the premise: `third` is being closed on the desk in view"
    );
    keyboard_left_pointer_right(&mut tiles);
    let ThreeTiles {
        mut desk,
        left,
        third,
        ..
    } = tiles;
    let before = heard(&desk);
    let token = genuine_token(&mut desk);
    activates(&mut desk, &third.surface, &token);
    still_on_the_left(
        &mut desk,
        &left,
        &before,
        third.pane.get(),
        "a window being closed on the desk in view asked to be brought forward",
    );
}

/// A selection of `pane` alone, carried by `to` from now, at
/// once: what `sol.group` and `sol.present_group` hold for one.
fn carry(desk: &mut Desk, pane: crate::pane::PaneId, to: crate::group::Shift) {
    let now = desk.state.clock.now();
    let selection = crate::group::Selection {
        members: vec![crate::group::Member::Window(pane.get())],
        on: None,
    };
    let _ = desk.state.groups.declare("under-test", selection, now);
    assert!(
        desk.state.groups.present(
            "under-test",
            to,
            now,
            Duration::ZERO,
            solium_animation::Curve::Linear,
        ),
        "the selection was declared"
    );
    frame(desk);
}

/// **#134's fifth review: a window whose selection fades it to
/// nothing where it is -- moved nowhere -- is refused before it
/// is focused**, as a desk carried a screen away is. Until then
/// only a displacement counted: it was focused, found to be
/// headed nowhere, and handed off to the tile under the pointer.
#[test]
fn a_genuine_activation_of_a_window_a_selection_fades_to_nothing_leaves_the_keyboard_exactly_where_it_was()
 {
    let mut tiles = three_tiles();
    carry(
        &mut tiles.desk,
        tiles.third.pane,
        crate::group::Shift {
            opacity: 0.0,
            ..crate::group::Shift::NONE
        },
    );
    assert!(
        !headed_on_stage(&tiles.desk.state, tiles.third.pane)
            && !tiles
                .desk
                .state
                .panes
                .get(tiles.third.pane)
                .is_some_and(Pane::leaving),
        "the premise: `third` is faded to nothing, and not being closed"
    );
    keyboard_left_pointer_right(&mut tiles);
    let ThreeTiles {
        mut desk,
        left,
        third,
        ..
    } = tiles;
    let before = heard(&desk);
    let token = genuine_token(&mut desk);
    activates(&mut desk, &third.surface, &token);
    still_on_the_left(
        &mut desk,
        &left,
        &before,
        third.pane.get(),
        "a window a selection fades to nothing asked to be brought forward",
    );
}

/// **#134's fifth review, finding 2: on a spring, a genuine
/// activation mid-slide of a window on the desk being switched
/// to takes the keyboard.** The desk is cleared from a screen
/// away, and a spring's progress at its end was within its
/// epsilon of 1 and not 1 -- so asked where the desk is at
/// `settling`, it was still a pixel or two off, a hidden
/// workspace, until the travel was retired: every window on the
/// desk being switched to was refused for the whole slide. A
/// spring lands exactly now (`solium_animation`'s
/// `a_spring_lands_exactly_on_its_target`), so at `settling` the
/// desk is where it is going, and the window still takes it.
#[test]
fn on_a_spring_a_genuine_activation_mid_slide_on_the_desk_being_switched_to_takes_the_keyboard() {
    let (mut desk, _working) = working_in(
        &format!(
            "require(\"config\").workspaces.motion = \
             {{ duration = 300, easing = \"spring\" }}\n\
             {SHIPPED_HEARING_FOCUS}"
        ),
        Some("super+t"),
    );
    let switch = |desk: &mut Desk, key: &str, showing_now: &str| {
        assert!(desk.state.trigger(key), "{key} was not handled");
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        frame(desk);
        assert_eq!(showing(desk), showing_now, "the premise: {key} switched");
    };
    switch(&mut desk, "super+2", "2");
    let one = desk.open_surface();
    let two = desk.open_surface();
    desk.answer(&one);
    desk.answer(&two);
    switch(&mut desk, "super+1", "1");
    assert!(
        workspace_of(&desk, one.pane) == "2"
            && workspace_of(&desk, two.pane) == "2"
            && !headed_on_stage(&desk.state, one.pane),
        "the premise: two windows parked on workspace 2"
    );

    // And back to it, a tenth of a second into the slide.
    assert!(desk.state.trigger("super+2"), "super+2 was not handled");
    desk.state.clock.advance(Duration::from_millis(100));
    frame(&mut desk);
    let had = desk.focused();
    let asking = if had == one.pane { &two } else { &one };
    let late = desk
        .state
        .panes
        .get(asking.pane)
        .map(|pane| {
            let slot = desk.state.pane_outer(pane);
            desk.state
                .carried_at(pane, slot, desk.state.settling())
                .offset()
        })
        .unwrap_or_default();
    assert!(
        late.0.abs() < f64::EPSILON && late.1.abs() < f64::EPSILON,
        "the premise: at `settling` the spring has workspace 2 where it is going, \
         and here it is {late:?} off"
    );
    assert!(
        headed_on_stage(&desk.state, asking.pane),
        "the premise: the window asking is headed on stage"
    );

    let token = genuine_token(&mut desk);
    activates(&mut desk, &asking.surface, &token);
    let asking = window_of(&desk, asking.pane);
    assert_eq!(
        desk.state.focused_window(),
        Some(asking.clone()),
        "a window on the desk being switched to asked to be brought forward mid-slide \
         and was refused"
    );
    typed_into(&mut desk, &asking, "a key typed after it did not reach it");
}

/// How thick the panel across the slide is in
/// [`under_a_panel`]: more than the tiling gap, so a tile on
/// the desk carried towards it shows under it.
const PANEL: i32 = 64;

/// A panel [`PANEL`] deep across the slide -- along the right
/// edge of the monitor for a horizontal arrangement, the top
/// for a vertical one -- holding it as its exclusive zone, the
/// way [`bar`] makes a top bar.
fn a_panel_across(
    desk: &mut Desk,
    vertical: bool,
) -> (
    wl_surface::WlSurface,
    zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
) {
    use zwlr_layer_surface_v1::Anchor;
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let shell = desk
        .client
        .layer_shell
        .clone()
        .expect("zwlr_layer_shell_v1 bound");
    let surface = compositor.create_surface(&desk.qh, ());
    let layer = shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Top,
        "activation-test-panel".to_string(),
        &desk.qh,
        (),
    );
    if vertical {
        layer.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        layer.set_size(0, PANEL.unsigned_abs());
    } else {
        layer.set_anchor(Anchor::Right | Anchor::Top | Anchor::Bottom);
        layer.set_size(PANEL.unsigned_abs(), 0);
    }
    layer.set_exclusive_zone(PANEL);
    surface.commit();
    desk.pump();
    (surface, layer)
}

/// Where a pane's selections are headed, as
/// `carried_by_a_selection` asks.
fn bound_for(desk: &Desk, pane: crate::pane::PaneId) -> (f64, f64) {
    let monitor = desk.state.groups.names_monitors().then_some("reflow-test");
    desk.state
        .groups
        .bound_for_window(pane.get(), monitor)
        .offset()
}

/// Tiling, one monitor, the shipped workspaces at `spread = 1.0`
/// in a row or a column of three, and a panel across the
/// slide. `before` is on workspace 1, carried left or up, with
/// the window the fixture started in; `after` and
/// `beside_after` are on workspace 3, carried right or down;
/// and workspace 2 is in view, with the keyboard on `left` and
/// the pointer resting on `right`, for the reason
/// [`a_strip_left_behind_on_workspace_1`] gives. Every animation
/// landed.
///
/// Of the three windows beside, one at least shows under the
/// panel and one is off the screen, which are the two ways the
/// regression went.
struct UnderAPanel {
    desk: Desk,
    before: Opened,
    after: Opened,
    beside_after: Opened,
    left: Window,
    _panel: (
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    ),
}

fn under_a_panel(vertical: bool) -> UnderAPanel {
    let arrangement = if vertical { "vertical" } else { "horizontal" };
    let (mut desk, _working) = working_in(
        &format!(
            "local config = require(\"config\")\n\
             config.workspaces.arrangement = \"{arrangement}\"\n\
             config.workspaces.columns = 3\n\
             config.workspaces.rows = 3\n\
             config.workspaces.spread = 1.0\n\
             {SHIPPED_HEARING_FOCUS}"
        ),
        Some("super+t"),
    );
    let panel = a_panel_across(&mut desk, vertical);
    let output = desk
        .state
        .space
        .outputs()
        .next()
        .cloned()
        .expect("the fixture has a monitor");
    let wanted = if vertical {
        Rectangle::new((0, PANEL).into(), (1920, 1080 - PANEL).into())
    } else {
        Rectangle::new((0, 0).into(), (1920 - PANEL, 1080).into())
    };
    assert_eq!(
        desk.state.work_area_on(&output),
        Some(wanted),
        "{arrangement}: the premise: the panel takes its thickness off the work \
         area, across the slide"
    );

    let settle = |desk: &mut Desk| {
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        frame(desk);
    };
    let before = desk.open_surface();
    desk.answer(&before);
    settle(&mut desk);
    assert!(desk.state.trigger("super+3"), "super+3 was not handled");
    settle(&mut desk);
    let after = desk.open_surface();
    let beside_after = desk.open_surface();
    desk.answer(&after);
    desk.answer(&beside_after);
    settle(&mut desk);
    assert!(desk.state.trigger("super+2"), "super+2 was not handled");
    settle(&mut desk);
    let left = desk.open_surface();
    let right = desk.open_surface();
    desk.answer(&left);
    desk.answer(&right);
    settle(&mut desk);
    assert_eq!(
        (
            showing(&desk),
            workspace_of(&desk, before.pane),
            workspace_of(&desk, after.pane),
            workspace_of(&desk, beside_after.pane),
            workspace_of(&desk, left.pane),
            workspace_of(&desk, right.pane),
        ),
        (
            "2".to_owned(),
            "1".to_owned(),
            "3".to_owned(),
            "3".to_owned(),
            "2".to_owned(),
            "2".to_owned()
        ),
        "{arrangement}: the premise: the view on workspace 2, `before` on 1, \
         `after` and `beside_after` on 3"
    );

    // Parked by the work area, which is the situation under
    // test: the whole screen carried that far still overlaps
    // the screen by the panel's thickness.
    let across = if vertical {
        (0.0, f64::from(1080 - PANEL))
    } else {
        (f64::from(1920 - PANEL), 0.0)
    };
    assert_eq!(
        (bound_for(&desk, before.pane), bound_for(&desk, after.pane)),
        ((-across.0, -across.1), across),
        "{arrangement}: the premise: each desk beside is parked a work area away"
    );
    let shows =
        [&before, &after, &beside_after].map(|beside| headed_on_stage(&desk.state, beside.pane));
    assert!(
        shows.contains(&true) && shows.contains(&false),
        "{arrangement}: the premise: of `before`, `after` and `beside_after`, one \
         shows under the panel and one is off the screen, and here they are \
         {shows:?}, living at {:?}",
        [&before, &after, &beside_after].map(|beside| desk.placed(beside.pane)),
    );
    assert!(
        headed_on_stage(&desk.state, left.pane) && headed_on_stage(&desk.state, right.pane),
        "{arrangement}: the premise: both of workspace 2's tiles are on screen"
    );

    let over_right = desk.placed(right.pane);
    point_at(
        &mut desk.state,
        (
            f64::from(over_right.loc.x + over_right.size.w / 2),
            f64::from(over_right.loc.y + over_right.size.h / 2),
        ),
    );
    let left = window_of(&desk, left.pane);
    desk.state.focus_window(
        &left,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    typed_into(&mut desk, &left, "the premise: typing reaches `left`");
    UnderAPanel {
        desk,
        before,
        after,
        beside_after,
        left,
        _panel: panel,
    }
}

/// A genuine activation of each window beside in turn: each
/// refused, and the keyboard exactly where it was.
fn neither_desk_beside_takes_the_keyboard(vertical: bool) {
    let arrangement = if vertical { "vertical" } else { "horizontal" };
    let UnderAPanel {
        mut desk,
        before,
        after,
        beside_after,
        left,
        ..
    } = under_a_panel(vertical);
    for (asking, which) in [
        (&before, "workspace 1"),
        (&after, "workspace 3"),
        (&beside_after, "workspace 3"),
    ] {
        let heard_before = heard(&desk);
        let token = genuine_token(&mut desk);
        activates(&mut desk, &asking.surface, &token);
        still_on_the_left(
            &mut desk,
            &left,
            &heard_before,
            asking.pane.get(),
            &format!(
                "{arrangement}: a window on {which}, parked a work area away, \
                 asked to be brought forward"
            ),
        );
    }
}

/// **#134's sixth review: a desk parked a work area away is a
/// hidden workspace, whatever a panel across the slide leaves
/// of its screen.** Until then `put_away` measured the desk
/// against its whole screen, and at `spread = 1.0` that still
/// overlapped the screen by the panel: a window on the desk to
/// the right showed under the panel and kept the keyboard, and
/// one on the desk to the left was focused, found off screen,
/// and handed off to the tile under the pointer.
#[test]
fn with_a_panel_across_a_horizontal_slide_a_genuine_activation_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
 {
    neither_desk_beside_takes_the_keyboard(false);
}

/// **The same in a column of workspaces, under a top bar**: the
/// desk above shows under the bar, and the one below is off the
/// bottom of the screen.
#[test]
fn with_a_bar_across_a_vertical_slide_a_genuine_activation_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
 {
    neither_desk_beside_takes_the_keyboard(true);
}

/// **And under the same panel, a genuine activation mid-slide
/// on the desk being switched to takes the keyboard**, in a row
/// and in a column: that desk is headed for nothing, however
/// far away it still is.
#[test]
fn with_a_panel_across_the_slide_a_genuine_activation_mid_slide_on_the_desk_being_switched_to_takes_the_keyboard()
 {
    for vertical in [false, true] {
        let arrangement = if vertical { "vertical" } else { "horizontal" };
        let UnderAPanel {
            mut desk,
            after,
            beside_after,
            ..
        } = under_a_panel(vertical);
        assert!(desk.state.trigger("super+3"), "super+3 was not handled");
        desk.state.clock.advance(Duration::from_millis(100));
        frame(&mut desk);
        let had = desk.focused();
        assert_eq!(
            workspace_of(&desk, had),
            "3",
            "{arrangement}: the premise: the switch gave the keyboard to \
             workspace 3"
        );
        let asking = if had == after.pane {
            &beside_after
        } else {
            &after
        };
        let now = desk.state.clock.now();
        let mid = desk
            .state
            .panes
            .get(asking.pane)
            .map(|pane| {
                let slot = desk.state.pane_outer(pane);
                desk.state.carried_at(pane, slot, now).offset()
            })
            .unwrap_or_default();
        assert!(
            mid != (0.0, 0.0) && bound_for(&desk, asking.pane) == (0.0, 0.0),
            "{arrangement}: the premise: workspace 3 is mid-slide, {mid:?} away, \
             and headed for nothing"
        );

        let token = genuine_token(&mut desk);
        activates(&mut desk, &asking.surface, &token);
        let asking = window_of(&desk, asking.pane);
        assert_eq!(
            desk.state.focused_window(),
            Some(asking.clone()),
            "{arrangement}: a window on the desk being switched to asked to be \
             brought forward mid-slide and was refused"
        );
        typed_into(
            &mut desk,
            &asking,
            &format!("{arrangement}: a key typed after it did not reach it"),
        );
    }
}

/// **With `follow_overflow = false`, no route hands the keyboard
/// to a window parked on a hidden workspace**: one opened by its
/// application, one launched that arrives with no token, with
/// its own token before its first frame and after, and a genuine
/// activation of the first of them. Each is parked on a
/// workspace of its own -- six of them, where the shipped four
/// would run out and let the fifth in beside the first -- and
/// the keyboard stays in the window the user is working in
/// throughout.
#[test]
fn with_follow_overflow_off_no_route_hands_the_keyboard_to_a_parked_window() {
    fn still_working(desk: &mut Desk, working: &Window, route: &str) {
        assert_eq!(
            desk.state.focused_window(),
            Some(working.clone()),
            "{route}: the keyboard went to a workspace nobody is looking at"
        );
        typed_into(
            desk,
            working,
            &format!("{route}: a key typed afterwards went somewhere else"),
        );
    }
    fn parked(desk: &Desk, pane: crate::pane::PaneId, route: &str) {
        assert!(
            !headed_on_stage(&desk.state, pane),
            "{route}: the premise: the window is parked a screen away, and it is \
             on workspace {} with the view on {}",
            workspace_of(desk, pane),
            showing(desk)
        );
    }

    let (mut desk, working) = working_in(
        &format!(
            "require(\"config\").workspaces.columns = 6\n{}",
            one_window_fills_a_workspace(false)
        ),
        Some("super+t"),
    );

    let opened = desk.open_surface();
    parked(&desk, opened.pane, "opened by its application");
    still_working(&mut desk, &working, "opened by its application");

    let pane = asked_for(&mut desk);
    parked(&desk, pane, "launched");
    let _ = arrives(&mut desk);
    still_working(&mut desk, &working, "launched, with no token");

    for sends in BOTH {
        let route = format!("launched, with its own token {sends:?}");
        let pane = asked_for(&mut desk);
        parked(&desk, pane, &route);
        let token = desk.state.launch_token(pane);
        let _ = arrives_activating(&mut desk, &token, sends);
        still_working(&mut desk, &working, &route);
    }

    let token = genuine_token(&mut desk);
    activates(&mut desk, &opened.surface, &token);
    still_working(&mut desk, &working, "a genuine activation");
}

/// **#134 third review, finding 2: a genuine activation of a
/// window on a hidden workspace focused it first, and then gave
/// the keyboard away** -- to whatever `settle_focus` picked,
/// which with two tiles on screen need not be the one that had
/// it.
///
/// Two tiles side by side, at a minimum two fit and three do
/// not. The keyboard is on the left one; the pointer rests on
/// the right one, which is what `settle_focus` asks first. The
/// window asking to be brought forward overflowed to workspace
/// 2. Focused and then handed off, the keyboard ended on the
/// tile under the pointer; and the parked window had been told
/// it had the keyboard, and the scripts that it was focused, on
/// the way.
#[test]
fn a_genuine_activation_of_a_window_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was()
{
    let (mut desk, _first) = working_in(
        &format!(
            "local config = require(\"config\")\n\
             config.tiling.minimum = {{ w = 900, h = 600 }}\n\
             config.tiling.follow_overflow = false\n\
             {SHIPPED_HEARING_FOCUS}"
        ),
        Some("super+t"),
    );
    let left = desk.focused();
    let right = desk.open_surface();
    desk.answer(&right);
    let left_window = window_of(&desk, left);
    let right_window = window_of(&desk, right.pane);
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    assert!(
        headed_on_stage(&desk.state, left) && headed_on_stage(&desk.state, right.pane),
        "the premise: two tiles on screen"
    );

    // The pointer on the right tile, the keyboard on the left.
    let over_right = desk.placed(right.pane);
    point_at(
        &mut desk.state,
        (
            f64::from(over_right.loc.x + over_right.size.w / 2),
            f64::from(over_right.loc.y + over_right.size.h / 2),
        ),
    );
    desk.state.focus_window(
        &left_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    typed_into(
        &mut desk,
        &left_window,
        "the premise: typing reaches the left tile",
    );

    let parked = desk.open_surface();
    assert_eq!(
        workspace_of(&desk, parked.pane),
        "2",
        "the premise: the third window had no room and went to workspace 2"
    );
    assert!(
        !headed_on_stage(&desk.state, parked.pane),
        "the premise: it is parked a screen away"
    );
    assert_eq!(
        desk.state.focused_window(),
        Some(left_window.clone()),
        "the premise: it opened without the keyboard"
    );

    let token = genuine_token(&mut desk);
    activates(&mut desk, &parked.surface, &token);
    assert_eq!(
        desk.state.focused_window(),
        Some(left_window.clone()),
        "an activation of a window nobody can see moved the keyboard off the \
         left tile -- to the right one (surface {}) if it went where the pointer is",
        surface_id(&right_window)
    );
    typed_into(
        &mut desk,
        &left_window,
        "a key typed after the activation went somewhere else",
    );
    let (heard, parked) = (heard(&desk), parked.pane.get().to_string());
    assert!(
        !heard.split(',').any(|id| id == parked),
        "the parked window was focused on the way, if only until the keyboard \
         was handed back. Heard: [{heard}]"
    );
}

/// **A refused activation is "this window wants you"** (03
/// §3.2.17, Ruling 18): the window asking to be brought forward
/// from a hidden workspace is urgent until it is focused.
#[test]
fn a_refused_activation_marks_the_window_urgent_until_it_is_focused() {
    let (mut desk, _first) = working_in(
        &format!(
            "local config = require(\"config\")\n\
             config.tiling.minimum = {{ w = 900, h = 600 }}\n\
             config.tiling.follow_overflow = false\n\
             {SHIPPED_HEARING_FOCUS}"
        ),
        Some("super+t"),
    );
    let left = desk.focused();
    let right = desk.open_surface();
    desk.answer(&right);
    let left_window = window_of(&desk, left);
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    assert!(
        headed_on_stage(&desk.state, left) && headed_on_stage(&desk.state, right.pane),
        "the premise: two tiles on screen"
    );

    // The pointer on the right tile, the keyboard on the left.
    let over_right = desk.placed(right.pane);
    point_at(
        &mut desk.state,
        (
            f64::from(over_right.loc.x + over_right.size.w / 2),
            f64::from(over_right.loc.y + over_right.size.h / 2),
        ),
    );
    desk.state.focus_window(
        &left_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    typed_into(
        &mut desk,
        &left_window,
        "the premise: typing reaches the left tile",
    );

    let parked = desk.open_surface();
    assert_eq!(
        workspace_of(&desk, parked.pane),
        "2",
        "the premise: the third window had no room and went to workspace 2"
    );
    assert!(
        !headed_on_stage(&desk.state, parked.pane),
        "the premise: it is parked a screen away"
    );
    assert_eq!(
        desk.state.focused_window(),
        Some(left_window.clone()),
        "the premise: it opened without the keyboard"
    );

    let token = genuine_token(&mut desk);
    desk.state.redraw = false;
    activates(&mut desk, &parked.surface, &token);
    let id = parked.pane.get();
    assert!(
        desk.state.urgent.contains(&id),
        "a refused activation did not mark the window urgent"
    );
    assert!(
        desk.state.redraw,
        "a window going urgent asked for no frame, so the models would not show it \
         until something unrelated redrew"
    );
    let parked_window = window_of(&desk, parked.pane);
    desk.state.focus_window(
        &parked_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    assert!(
        !desk.state.urgent.contains(&id),
        "focusing it did not clear urgent"
    );
}

/// **#134 third review, finding 3: a window claimed by its
/// launch's token into a pane parked on a hidden workspace kept
/// the keyboard it had.**
///
/// A launcher that forks and exits breaks the pid chain, so its
/// application opens a window of its own -- on screen, and
/// given the keyboard there, as any new window is -- and then
/// activates with the token `sol.spawn` handed it.
/// `claim_into` moves it into the window that was opened for the
/// launch, and that one is on workspace 2. It moved, and the
/// keyboard went with it.
#[test]
fn a_focused_window_claimed_into_a_pane_on_a_hidden_workspace_gives_the_keyboard_up() {
    let mut desk = Desk::new();
    desk.install(&one_window_fills_a_workspace(false));
    assert!(desk.state.trigger("super+t"));
    let (_in_the_way, toplevel, _) = desk.open();
    desk.pump();
    desk.pump();
    assert!(
        desk.client.keyboard.is_some(),
        "the client bound a keyboard"
    );

    // No pid, so nothing adopts the application by its
    // process: only the token can.
    let source = crate::pane::loading_source(None);
    let launched = desk.state.open_loading("app", None, source, None);
    assert_eq!(
        workspace_of(&desk, launched),
        "2",
        "the premise: the launch had no room and went to workspace 2"
    );
    // And the window that filled workspace 1 goes, so there is
    // room there for the application's own window.
    toplevel.destroy();
    desk.pump();
    frame(&mut desk);

    let own = desk.open_surface();
    let window = window_of(&desk, own.pane);
    assert_ne!(
        own.pane, launched,
        "the premise: it opened a window of its own"
    );
    assert_eq!(
        desk.state.focused_window(),
        Some(window.clone()),
        "the premise: that window opened on screen and has the keyboard"
    );

    let token = desk.state.launch_token(launched);
    activates(&mut desk, &own.surface, &token);
    assert_eq!(
        desk.state.panes.id_of(&window),
        Some(launched),
        "the premise: the token moved it into the window opened for the launch"
    );
    assert!(
        !headed_on_stage(&desk.state, launched),
        "the premise: and that window is parked a screen away"
    );
    assert_ne!(
        desk.state.focused_window(),
        Some(window.clone()),
        "the keyboard went with the window to a workspace nobody is looking at"
    );
    let before = desk.client.typed.len();
    types(&mut desk.state, KEY_A);
    desk.pump();
    assert!(
        desk.client.typed[before..]
            .iter()
            .all(|(on, _)| *on != Some(surface_id(&window))),
        "a key typed after the token reached the window nobody can see"
    );
}

/// **Two monitors side by side, #134's third review, finding
/// 1.** Everything above uses one monitor, where a workspace
/// parked a screen away is parked on no screen at all. With the
/// shipped `per_monitor`, `horizontal` and `spread = 1.06`, the
/// left monitor's workspace 2 is carried 2035 pixels right --
/// onto the right monitor, which the renderer never draws it on,
/// because a pane is drawn only on the monitors its slot is on.
/// Every "is this on screen" question answered against every
/// screen instead, and said yes.
///
/// The desk: tiling, one window fills a workspace, overflow does
/// not follow. `working` on the left monitor with the keyboard,
/// `right` on the right monitor, and `parked` opened after both
/// with no room on the left, so it is on the left monitor's
/// workspace 2 and drawn over `right`. Every client has answered
/// its tile, so each draws the whole of it, and the animations
/// have landed.
struct SideBySide {
    desk: Desk,
    working: Opened,
    right: Opened,
    parked: Opened,
}

/// The right monitor's rectangle.
fn right_screen() -> Rectangle<i32, Logical> {
    Rectangle::new((1920, 0).into(), (1920, 1080).into())
}

fn side_by_side_with_a_parked_window() -> SideBySide {
    side_by_side_with_a_parked_window_and("")
}

/// [`side_by_side_with_a_parked_window`], with `extra` loaded
/// after the shipped layouts -- `overview`, which `init.lua`
/// requires and the recorder here does not.
fn side_by_side_with_a_parked_window_and(extra: &str) -> SideBySide {
    let mut desk = Desk::side_by_side();
    desk.install(&format!("{}\n{extra}", one_window_fills_a_workspace(false)));
    // Floating first, to put a window on the right monitor by
    // hand: every new window maps at the origin, which is the
    // left one, and tiling then adopts it where it is.
    let right = desk.open_surface();
    desk.state
        .map_stacked(window_of(&desk, right.pane), (2020, 100), false);
    frame(&mut desk);
    assert!(desk.state.trigger("super+t"), "super+t was not handled");
    let working = desk.open_surface();
    desk.pump();
    desk.pump();
    assert!(
        desk.client.keyboard.is_some(),
        "the client bound a keyboard"
    );
    desk.answer(&right);
    desk.answer(&working);
    let parked = desk.open_surface();
    desk.answer(&parked);
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);

    let screen = right_screen();
    assert!(
        screen.contains_rect(desk.placed(right.pane))
            && !screen.overlaps(desk.placed(working.pane)),
        "the premise: `right` is tiled on the right monitor and `working` on \
         the left, at {:?} and {:?}",
        desk.placed(right.pane),
        desk.placed(working.pane)
    );
    assert_eq!(
        (
            workspace_of(&desk, parked.pane),
            showing(&desk),
            says(
                &desk,
                &format!("return tostring(require(\"workspaces\").on({RIGHT_SCREEN:?}))")
            )
        ),
        ("2".to_owned(), "1".to_owned(), "1".to_owned()),
        "the premise: `parked` is on workspace 2 and both monitors show 1"
    );
    let drawn = drawn_now(&desk.state, parked.pane, desk.state.settling()).rect;
    assert!(
        !screen.overlaps(desk.placed(parked.pane)) && screen.to_f64().overlaps(drawn),
        "the premise, and the situation under test: `parked` lives on the left \
         monitor at {:?} and is carried onto the right one, to {drawn:?}",
        desk.placed(parked.pane)
    );
    SideBySide {
        desk,
        working,
        right,
        parked,
    }
}

/// The pointer, moved to `at` as a device would move it.
fn point_at(state: &mut Solium, at: (f64, f64)) {
    let pointer = state.seat.get_pointer().expect(
        "the fixture's seat has a pointer; without one there is no pointer \
         arm to test",
    );
    pointer.motion(
        state,
        None,
        &smithay::input::pointer::MotionEvent {
            location: at.into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(state);
}

/// The user goes to the left monitor's workspace 2, works in
/// the parked window there, and comes back to workspace 1 --
/// which leaves the parked window stacked above `right`, as the
/// window last raised on that monitor before the switch away.
/// What a walk topmost-first meets before the window the right
/// monitor draws.
fn visit_the_parked_window(side: &mut SideBySide) {
    let desk = &mut side.desk;
    for (key, showing_now) in [("super+2", "2"), ("super+1", "1")] {
        assert!(desk.state.trigger(key), "{key} was not handled");
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        frame(desk);
        assert_eq!(showing(desk), showing_now, "the premise: {key} switched");
    }
    let working = window_of(desk, side.working.pane);
    assert!(
        heard(desk)
            .split(',')
            .any(|id| id == side.parked.pane.get().to_string())
            && desk.state.focused_window() == Some(working),
        "the premise: the parked window was worked in on workspace 2, and the \
         keyboard came back to `working` with the view"
    );
    assert!(
        above(&mut desk.state, side.parked.pane, side.right.pane),
        "the premise: the parked window is stacked above the right one, so a \
         walk topmost-first meets it first"
    );
}

/// The first thing any question about visibility has to get
/// right: `parked` is not on stage, and the windows that are
/// drawn are. And idle inhibition, which asks the same question
/// of the present: a video on the left monitor's hidden
/// workspace does not hold the machine awake from the right.
#[test]
fn on_two_monitors_a_window_on_the_left_monitors_hidden_workspace_is_not_on_stage() {
    let SideBySide {
        desk,
        working,
        right,
        parked,
    } = side_by_side_with_a_parked_window();
    let surface = |pane| {
        window_of(&desk, pane)
            .wl_surface()
            .map(std::borrow::Cow::into_owned)
            .expect("a mapped client has a surface")
    };
    // Every answer asked before any is asserted, so a failure
    // says which question is wrong rather than only the first.
    let on_stage = |pane| headed_on_stage(&desk.state, pane);
    let visible = |pane| desk.state.surface_is_visible(&surface(pane));
    assert_eq!(
        (
            on_stage(parked.pane),
            on_stage(working.pane),
            on_stage(right.pane),
            visible(parked.pane),
            visible(right.pane),
        ),
        (false, true, true, false, true),
        "(parked on stage, working on stage, right on stage, parked visible to \
         idle inhibition, right visible to it): a window on the left monitor's \
         hidden workspace counted as on screen because it is carried over the \
         right monitor, which never draws it -- or a window that is drawn did not"
    );
}

/// **`offer_keyboard`**: the window that opened onto the left
/// monitor's hidden workspace did not take the keyboard.
#[test]
fn on_two_monitors_a_window_opening_onto_the_left_monitors_hidden_workspace_does_not_take_the_keyboard()
 {
    let SideBySide {
        mut desk,
        working,
        parked,
        ..
    } = side_by_side_with_a_parked_window();
    let working = window_of(&desk, working.pane);
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "the window that opened onto the left monitor's hidden workspace took \
         the keyboard"
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after it opened went somewhere else",
    );
    let (heard, parked) = (heard(&desk), parked.pane.get().to_string());
    assert!(
        !heard.split(',').any(|id| id == parked),
        "and the scripts heard it focused. Heard: [{heard}]"
    );
}

/// **`settle_focus`'s topmost arm**: the last window on the
/// left monitor closes, with the pointer over nothing, and the
/// keyboard goes to the window on the right monitor -- not to
/// the parked one, which is stacked above it. Live on `stage`
/// before #134, whenever the last window on a monitor closed.
#[test]
fn on_two_monitors_closing_the_last_window_on_the_left_hands_the_keyboard_to_the_right_monitor() {
    let mut side = side_by_side_with_a_parked_window();
    visit_the_parked_window(&mut side);
    let SideBySide {
        mut desk,
        working,
        right,
        parked,
    } = side;
    let right_window = window_of(&desk, right.pane);
    desk.state.close_pane(working.pane);
    desk.ask();
    assert_eq!(
        desk.state.focused_window(),
        Some(right_window.clone()),
        "closing the last window on the left monitor handed the keyboard to the \
         left monitor's hidden workspace (surface {}) rather than to the window \
         on the right monitor",
        surface_id(&window_of(&desk, parked.pane))
    );
    typed_into(
        &mut desk,
        &right_window,
        "a key typed after the close went somewhere else",
    );
}

/// **`settle_focus`'s pointer arm**: a pointer resting on the
/// right monitor, at a pixel the parked window is carried over,
/// hands the keyboard to the window the right monitor draws
/// there.
#[test]
fn on_two_monitors_a_pointer_on_the_right_monitor_does_not_settle_the_keyboard_on_the_left_monitors_hidden_workspace()
 {
    let mut side = side_by_side_with_a_parked_window();
    visit_the_parked_window(&mut side);
    let SideBySide {
        mut desk, right, ..
    } = side;
    let right_window = window_of(&desk, right.pane);
    point_at(&mut desk.state, (2500.0, 500.0));
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    desk.state.settle_focus();
    assert_eq!(
        desk.state.focused_window(),
        Some(right_window.clone()),
        "a pointer resting on the right monitor handed the keyboard to the \
         window carried over it from the left monitor's hidden workspace"
    );
    typed_into(
        &mut desk,
        &right_window,
        "a key typed afterwards went somewhere else",
    );
}

/// **The hit tests**: a press on the right monitor, at a pixel
/// the parked window is carried over, reaches the window the
/// right monitor draws there -- `window_under` for
/// click-to-focus, `surface_under` for the event itself, and
/// `chrome_under` for the parked window's edge, which is there
/// only in arithmetic. Otherwise it is an input black hole: a
/// press lands in a window nobody can see, and focus follows it.
#[test]
fn on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws() {
    let mut side = side_by_side_with_a_parked_window();
    visit_the_parked_window(&mut side);
    let SideBySide {
        desk,
        right,
        parked,
        ..
    } = side;
    let right_window = window_of(&desk, right.pane);
    let now = desk.state.clock.now();
    let drawn = drawn_now(&desk.state, parked.pane, now).rect;
    let point = Point::<f64, Logical>::from((2500.0, 500.0));
    assert!(
        drawn.contains(point),
        "the premise: the parked window is carried over {point:?}, to {drawn:?}"
    );
    // The parked window's carried resize border, twice: just
    // inside its left edge, over the middle of the right window;
    // and just outside its top edge, over the bare gap above
    // both -- where a border that is only a claim, a `Halo`,
    // would win for want of anything drawn beneath it.
    let edge = Point::<f64, Logical>::from((drawn.loc.x + 2.0, 500.0));
    let halo = Point::<f64, Logical>::from((2500.0, drawn.loc.y - 4.0));

    // Everything asked before anything is asserted, so a
    // failure says which walk is wrong rather than only the
    // first.
    let clicked = desk.state.window_under(point).map(|(window, _)| window);
    let delivered = desk.state.surface_under(point).map(|(surface, _)| surface);
    let parked_edge = |at| {
        desk.state
            .chrome_under(at)
            .is_some_and(|under| under.pane == parked.pane)
    };
    assert_eq!(
        (
            clicked.as_ref().map(surface_id),
            delivered.map(|surface| surface.id().protocol_id()),
            parked_edge(edge),
            parked_edge(halo),
        ),
        (
            Some(surface_id(&right_window)),
            Some(surface_id(&right_window)),
            false,
            false
        ),
        "(click-to-focus, the pointer event, the parked window's edge over the \
         right window, and over bare desktop): a press on the right monitor \
         reached the window carried over it from the left monitor's hidden \
         workspace, surface {}",
        surface_id(&window_of(&desk, parked.pane))
    );
}

/// **An activation**: of the parked window, it leaves the
/// keyboard where it was; of the window on the right monitor, it
/// takes the keyboard, as it should.
#[test]
fn on_two_monitors_an_activation_of_the_left_monitors_hidden_workspace_leaves_the_keyboard_where_it_was()
 {
    let SideBySide {
        mut desk,
        working,
        right,
        parked,
    } = side_by_side_with_a_parked_window();
    let working = window_of(&desk, working.pane);
    let token = genuine_token(&mut desk);
    activates(&mut desk, &parked.surface, &token);
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "an activation took the keyboard to the left monitor's hidden workspace"
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after the activation went somewhere else",
    );
    let (heard, id) = (heard(&desk), parked.pane.get().to_string());
    assert!(
        !heard.split(',').any(|each| each == id),
        "and the parked window was focused on the way. Heard: [{heard}]"
    );

    let token = genuine_token(&mut desk);
    activates(&mut desk, &right.surface, &token);
    let right_window = window_of(&desk, right.pane);
    assert_eq!(
        desk.state.focused_window(),
        Some(right_window.clone()),
        "the window on the right monitor asked to be brought forward and was refused"
    );
    typed_into(
        &mut desk,
        &right_window,
        "a key typed after it did not reach it",
    );
}

/// **`sol.window_at`, #134's fourth review, finding 1: asked
/// from Lua, it answers what the Rust hit test answers.** At a
/// pixel on the right monitor the parked window is carried over,
/// the window the right monitor draws there; and past the right
/// window's tile, where the parked window is carried and the
/// right monitor draws nothing, nothing. It matched the drawn
/// rectangle against the point and nothing else, so it found the
/// parked window at both -- stacked above the right one, and the
/// only window whose rectangle reaches the second.
#[test]
fn on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws() {
    let mut side = side_by_side_with_a_parked_window();
    visit_the_parked_window(&mut side);
    let SideBySide {
        desk,
        right,
        parked,
        ..
    } = side;
    let now = desk.state.clock.now();
    let carried = drawn_now(&desk.state, parked.pane, now).rect;
    let tile = desk.placed(right.pane);
    let screen = right_screen();
    let over = Point::<f64, Logical>::from((2500.0, 500.0));
    // Between the right window's tile and the monitor's edge:
    // the gap the layout leaves there.
    let bare = Point::<f64, Logical>::from((
        f64::from(tile.loc.x + tile.size.w + screen.loc.x + screen.size.w) / 2.0,
        500.0,
    ));
    assert!(
        carried.contains(over)
            && carried.contains(bare)
            && tile.to_f64().contains(over)
            && !tile.to_f64().contains(bare)
            && screen.to_f64().contains(bare),
        "the premise: the parked window is carried over {over:?}, which is on the \
         right window's tile at {tile:?}, and over {bare:?}, which is on the right \
         monitor and on no tile; it is carried to {carried:?}"
    );

    let from_lua = (
        window_at_from_lua(&desk, over),
        window_at_from_lua(&desk, bare),
    );
    let from_rust = (
        desk.state
            .window_under(over)
            .and_then(|(window, _)| desk.state.panes.id_of(&window))
            .map(|pane| pane.get().to_string()),
        desk.state
            .window_under(bare)
            .and_then(|(window, _)| desk.state.panes.id_of(&window))
            .map(|pane| pane.get().to_string()),
    );
    assert_eq!(
        from_lua,
        (right.pane.get().to_string(), "nil".to_owned()),
        "(over the right window, over the bare gap beside it): `sol.window_at` \
         found the window carried over the right monitor from the left one's \
         hidden workspace, window {}",
        parked.pane.get()
    );
    assert_eq!(
        from_rust,
        (Some(right.pane.get().to_string()), None),
        "and the Rust hit test gives another answer: the two are not one rule"
    );
}

/// **The overview, as shipped: a click on empty space on the
/// right monitor leaves it, and the keyboard where it was.**
/// The reviewer's own route to finding 1. Overview shrinks the
/// windows in view into a grid and leaves the parked one where
/// its desk carries it, over the right monitor; a click where no
/// thumbnail is drawn is how it is left, and `overview.lua` hands
/// whatever `sol.window_at` finds there to `sol.focus`. That was
/// the parked window, which took the keyboard on a desk nobody
/// can see.
#[test]
fn on_two_monitors_an_overview_click_on_the_right_monitor_leaves_the_keyboard_on_screen() {
    let mut side = side_by_side_with_a_parked_window_and("require(\"overview\")");
    visit_the_parked_window(&mut side);
    let SideBySide {
        mut desk,
        working,
        right,
        parked,
    } = side;
    let working = window_of(&desk, working.pane);
    assert!(
        desk.state.trigger("super+space"),
        "super+space was not handled"
    );
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);
    assert!(desk.state.script_grab, "the premise: overview owns input");

    // A pixel of the right monitor the parked window is carried
    // over and the right window's thumbnail is not.
    let now = desk.state.clock.now();
    let carried = drawn_now(&desk.state, parked.pane, now).rect;
    let thumbnail = drawn_now(&desk.state, right.pane, now).rect;
    let screen = right_screen();
    let empty = (screen.loc.y..screen.loc.y + screen.size.h)
        .step_by(4)
        .flat_map(|y| {
            (screen.loc.x..screen.loc.x + screen.size.w)
                .step_by(4)
                .map(move |x| Point::<f64, Logical>::from((f64::from(x), f64::from(y))))
        })
        .find(|point| carried.contains(*point) && !thumbnail.contains(*point))
        .expect(
            "the premise: somewhere on the right monitor the parked window is \
             carried over and the right window's thumbnail is not",
        );

    let before = heard(&desk);
    assert!(
        desk.state.trigger_click(empty.x, empty.y),
        "overview did not take the click"
    );
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);
    assert!(
        !desk.state.script_grab,
        "the premise: the click left overview"
    );
    assert_eq!(
        desk.state.focused_window(),
        Some(working.clone()),
        "a click on empty space on the right monitor, at {empty:?}, handed the \
         keyboard to the window carried there from the left monitor's hidden \
         workspace, surface {}",
        surface_id(&window_of(&desk, parked.pane))
    );
    typed_into(
        &mut desk,
        &working,
        "a key typed after the click went somewhere else",
    );
    let after = heard(&desk);
    let since = after.get(before.len()..).unwrap_or_default();
    let parked = parked.pane.get().to_string();
    assert!(
        !since.split(',').any(|id| id == parked),
        "and the scripts heard the parked window focused. Heard since the \
         click: [{since}]"
    );
}

/// Every answer `sol.window_at` gives a layout, oldest first --
/// the target a split or a drop was handed, which the tree then
/// looks past when it holds no such window, so no arrangement
/// shows it. Installed ahead of the layouts, which look `sol.window_at`
/// up each time they call it.
/// `on_two_monitors_tiling_splits_and_both_layouts_drop_onto_what_the_left_monitor_draws`.
const RECORDS_WINDOW_AT: &str = "asked = {}\n\
     local window_at = sol.window_at\n\
     sol.window_at = function(x, y, skip)\n\
         local id = window_at(x, y, skip)\n\
         asked[#asked + 1] = tostring(id)\n\
         return id\n\
     end";

/// The last answer [`RECORDS_WINDOW_AT`] heard, and forget them all.
fn last_asked(desk: &Desk) -> String {
    says(
        desk,
        "local last = asked[#asked]; asked = {}; return tostring(last)",
    )
}

/// Two monitors side by side under the layout `key` switches on,
/// at the shipped settings: `a` and `b` side by side on the left
/// monitor, `right` on the right one -- and then the right
/// monitor switched to its workspace 2, so `right` is on a
/// hidden desk carried a screen and a bit to the left, over
/// `a`, and stacked above it: it had the keyboard last, and no
/// layout has placed anything since. Every animation landed.
///
/// A fixture of its own rather than
/// [`side_by_side_with_a_parked_window`], which is tiling's:
/// only tiling's overflow parks a window on a hidden workspace,
/// and scrolling answers a focus with a sweep that places -- and
/// so raises -- every window in view back over a hidden one.
/// Here the last thing to happen is a switch, which places
/// nothing.
/// `on_two_monitors_tiling_splits_and_both_layouts_drop_onto_what_the_left_monitor_draws`.
struct Carried {
    desk: Desk,
    a: Opened,
    b: Opened,
    right: Opened,
    /// A point on `a`, and inside where `right` is carried.
    over_a: Point<f64, Logical>,
}

fn right_monitor_switched_away(key: &str) -> Carried {
    let mut desk = Desk::side_by_side();
    desk.install(&format!("{RECORDS_WINDOW_AT}\n{SHIPPED_HEARING_FOCUS}"));
    let right = desk.open_surface();
    desk.state
        .map_stacked(window_of(&desk, right.pane), (2020, 100), false);
    frame(&mut desk);
    assert!(desk.state.trigger(key), "{key} was not handled");
    let a = desk.open_surface();
    let b = desk.open_surface();
    for opened in [&right, &a, &b] {
        desk.answer(opened);
    }
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);
    let screen = right_screen();
    assert!(
        screen.contains_rect(desk.placed(right.pane))
            && !screen.overlaps(desk.placed(a.pane))
            && !screen.overlaps(desk.placed(b.pane))
            && !desk.placed(a.pane).overlaps(desk.placed(b.pane)),
        "{key}: the premise: `right` on the right monitor, `a` and `b` side by \
         side on the left, at {:?}, {:?} and {:?}",
        desk.placed(right.pane),
        desk.placed(a.pane),
        desk.placed(b.pane)
    );

    point_at(&mut desk.state, (2500.0, 500.0));
    desk.state.focus_window(
        &window_of(&desk, right.pane),
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    assert!(desk.state.trigger("super+2"), "super+2 was not handled");
    // Carried away, `right` gave the keyboard up (#403), and the
    // window that took it was raised. Put back over `a`, where it
    // was focused, so a walk topmost first meets it first.
    desk.state
        .space
        .raise_element(&window_of(&desk, right.pane), false);
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);
    assert_eq!(
        (
            showing(&desk),
            says(
                &desk,
                &format!("return tostring(require(\"workspaces\").on({RIGHT_SCREEN:?}))")
            ),
            workspace_of(&desk, right.pane),
        ),
        ("1".to_owned(), "2".to_owned(), "1".to_owned()),
        "{key}: the premise: the right monitor shows workspace 2, the left one 1, \
         and `right` is on 1"
    );
    let carried = drawn_now(&desk.state, right.pane, now).rect;
    let on_a = desk.placed(a.pane).to_f64().intersection(carried).expect(
        "the premise: `right` is carried over `a`, which is what the left \
         monitor draws there",
    );
    let over_a = Point::<f64, Logical>::from((
        on_a.loc.x + on_a.size.w / 2.0,
        on_a.loc.y + on_a.size.h / 2.0,
    ));
    assert!(
        above(&mut desk.state, right.pane, a.pane),
        "{key}: the premise: `right` is stacked above `a`, so a walk topmost \
         first meets it first"
    );
    let _ = last_asked(&desk);
    Carried {
        desk,
        a,
        b,
        right,
        over_a,
    }
}

/// **Tiling's split at open and both layouts' drops, #134's
/// fourth review, finding 1: each is handed the window the
/// monitor under the point draws**, and not a window on a
/// hidden desk carried over it. The split and tiling's drop
/// hand the target to a tree that holds no such window, which
/// then falls back on its own hit test, so only the target
/// itself shows the fault; scrolling's drop checks its strip
/// holds the target, and so dropped onto `a` went nowhere.
#[test]
fn on_two_monitors_tiling_splits_and_both_layouts_drop_onto_what_the_left_monitor_draws() {
    let Carried {
        mut desk,
        a,
        right,
        over_a,
        ..
    } = right_monitor_switched_away("super+t");
    point_at(&mut desk.state, (over_a.x, over_a.y));
    let _ = desk.open_surface();
    let at_open = last_asked(&desk);

    let Carried {
        mut desk,
        a: tiled_a,
        b,
        over_a,
        ..
    } = right_monitor_switched_away("super+t");
    desk.state
        .trigger_drop(&window_of(&desk, b.pane), over_a.x, over_a.y);
    let tiling_drop = last_asked(&desk);

    let Carried {
        mut desk,
        a: strip_a,
        b: strip_b,
        over_a,
        ..
    } = right_monitor_switched_away("super+s");
    desk.state
        .trigger_drop(&window_of(&desk, strip_b.pane), over_a.x, over_a.y);
    let scrolling_drop = last_asked(&desk);
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);
    let joined = desk.placed(strip_b.pane).loc.x == desk.placed(strip_a.pane).loc.x;

    assert_eq!(
        (at_open, tiling_drop, scrolling_drop, joined),
        (
            a.pane.get().to_string(),
            tiled_a.pane.get().to_string(),
            strip_a.pane.get().to_string(),
            true
        ),
        "(tiling's split target at open, tiling's drop target, scrolling's drop \
         target, whether the window dropped in scrolling joined `a`'s column): a \
         layout was handed the window carried over the left monitor from the \
         right one's hidden workspace, window {}",
        right.pane.get()
    );
}

mod relaunch;
