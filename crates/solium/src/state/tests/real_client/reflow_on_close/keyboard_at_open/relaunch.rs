//! **#177: launching an application that is already running.**
//!
//! Firefox, Telegram, any GTK or Qt application with one
//! instance: a second launch is passed on to the instance that is
//! running, which brings forward the window it already has, with
//! the launch's own token. `claim_into` took that for the
//! launch's arrival and moved the window into the loading pane:
//! it left its workspace for the one in view, changed its id, and
//! the scripts heard it close. The client here does what spike
//! SVC-S9's GTK 4 application did, measured nested: an existing
//! toplevel activates with the token the launch was handed.

use super::*;

/// The shipped layouts, as `init.lua` requires them, with
/// every `open`, `focus`, `close` and `activate` the scripts
/// hear written down in `told`, as `"<event> <id>"` and an
/// `activate`'s reason after its id.
const SHIPPED_TELLING: &str = "require(\"modes\")\n\
     require(\"workspaces\")\n\
     require(\"tiling\")\n\
     require(\"scrolling\")\n\
     told = {}\n\
     for _, name in ipairs({ \"open\", \"focus\", \"close\" }) do\n\
         sol.on(name, function(id) told[#told + 1] = name .. \" \" .. id end)\n\
     end\n\
     sol.on(\"activate\", function(id, why)\n\
         told[#told + 1] = \"activate \" .. id .. \" \" .. tostring(why)\n\
     end)";

/// What the scripts were told, oldest first.
fn told(desk: &Desk) -> Vec<String> {
    says(desk, "return table.concat(told, \",\")")
        .split(',')
        .filter(|event| !event.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The modes a test runs in, and what each is called in its
/// messages: floating, the startup default, then the two
/// layouts.
const MODES: [(Option<&str>, &str); 3] = [
    (None, "floating"),
    (Some("super+t"), "tiling"),
    (Some("super+s"), "scrolling"),
];

/// Let every animation land.
fn settled(desk: &mut Desk) {
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(desk);
}

/// The application, running: its window open on `desk` in
/// the mode `key` switches on, with the keyboard, having
/// answered its tile where a layout gave it one, and every
/// animation landed.
fn running_on(mut desk: Desk, key: Option<&str>) -> (Desk, Opened) {
    desk.install(SHIPPED_TELLING);
    if let Some(key) = key {
        assert!(desk.state.trigger(key), "{key} was not handled");
    }
    let app = desk.open_surface();
    // Twice more, for the keyboard: see `working_in`.
    desk.pump();
    desk.pump();
    assert!(
        desk.client.keyboard.is_some(),
        "the client bound a keyboard"
    );
    if desk
        .state
        .panes
        .get(app.pane)
        .and_then(Pane::placed)
        .is_some()
    {
        desk.answer(&app);
    }
    settled(&mut desk);
    assert_eq!(
        desk.state.focused_window(),
        Some(window_of(&desk, app.pane)),
        "the premise: the application's window has the keyboard"
    );
    (desk, app)
}

/// [`running_on`] one monitor.
fn running(key: Option<&str>) -> (Desk, Opened) {
    running_on(Desk::new(), key)
}

/// The application's window, sent to workspace 2 while the
/// view stays on 1.
fn sent_away(desk: &mut Desk, app: &Opened, mode: &str) {
    assert!(
        desk.state.trigger("super+shift+2"),
        "{mode}: super+shift+2 was not handled"
    );
    settled(desk);
    assert_eq!(
        (workspace_of(desk, app.pane), showing(desk)),
        ("2".to_owned(), "1".to_owned()),
        "{mode}: the premise: the application's window is on workspace 2 and \
         the view on 1"
    );
    assert!(
        !headed_on_stage(&desk.state, app.pane),
        "{mode}: the premise: the application's window is out of sight"
    );
}

/// What `sol.spawn` does before it forks, for a program
/// whose process nothing will match: an application that
/// keeps one instance hands the launch to that instance and
/// exits, so only the token can find it.
fn launched(desk: &mut Desk) -> crate::pane::PaneId {
    let source = crate::pane::loading_source(None);
    desk.state.open_loading("app", None, source, None)
}

/// The application, running in `app`, is launched again:
/// the launch opens its window, and the instance that is
/// running brings `app` forward with the launch's token.
/// Returns the launch's window.
fn relaunched(desk: &mut Desk, app: &Opened, mode: &str) -> crate::pane::PaneId {
    let launch = launched(desk);
    assert!(
        app.pane < launch,
        "{mode}: the premise: the application's window was there before the \
         launch"
    );
    let token = desk.state.launch_token(launch);
    activates(desk, &app.surface, &token);
    launch
}

/// **On another workspace, the window stays there with its
/// id, and the view goes to it, focused**, in every mode.
/// Before, it was moved into the window opened for the
/// launch, on the workspace in view, under that window's id.
/// The view is `workspaces.lua`'s `activate` handler: the
/// compositor refuses to focus a window on a workspace
/// nobody is looking at, so dissolving the launch's window
/// alone left an empty screen and the application where it
/// was, by SVC-S9's frames.
#[test]
fn relaunching_an_application_whose_window_is_on_another_workspace_keeps_it_there_and_brings_it_into_view()
 {
    for (key, mode) in MODES {
        let (mut desk, app) = running(key);
        let window = window_of(&desk, app.pane);
        sent_away(&mut desk, &app, mode);

        let launch = relaunched(&mut desk, &app, mode);
        settled(&mut desk);
        assert_eq!(
            (
                desk.state.panes.id_of(&window),
                workspace_of(&desk, app.pane),
                showing(&desk)
            ),
            (Some(app.pane), "2".to_owned(), "2".to_owned()),
            "{mode}: (the window's pane, its workspace, the workspace in view) \
             after the relaunch: the window was moved into the one opened for \
             the launch, pane {}, or the view did not go to it",
            launch.get()
        );
        assert!(
            desk.state.panes.get(launch).is_none_or(Pane::leaving),
            "{mode}: the window opened for the launch is still there"
        );
        assert!(
            headed_on_stage(&desk.state, app.pane),
            "{mode}: the view went to workspace 2 and the window is still not \
             on screen"
        );
        assert_eq!(
            desk.state.focused_window(),
            Some(window.clone()),
            "{mode}: the relaunched application's window was brought into view \
             without the keyboard"
        );
        typed_into(
            &mut desk,
            &window,
            &format!("{mode}: a key typed after the relaunch went somewhere else"),
        );
    }
}

/// **On the other monitor's hidden workspace, that monitor
/// switches**, with workspaces per monitor as shipped: the
/// view goes to the window on its own screen, and the screen
/// the pointer is on goes on showing what it was.
#[test]
fn relaunching_an_application_whose_window_is_on_the_other_monitors_hidden_workspace_switches_that_monitor()
 {
    let (mut desk, app) = running_on(Desk::side_by_side(), None);
    let window = window_of(&desk, app.pane);
    desk.state.map_stacked(window.clone(), (2020, 100), false);
    frame(&mut desk);
    point_at(&mut desk.state, (500.0, 500.0));
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    assert!(desk.state.trigger("super+shift+2"));
    settled(&mut desk);
    let right = format!("return tostring(require(\"workspaces\").on({RIGHT_SCREEN:?}))");
    assert_eq!(
        (
            workspace_of(&desk, app.pane),
            says(&desk, &right),
            showing(&desk)
        ),
        ("2".to_owned(), "1".to_owned(), "1".to_owned()),
        "the premise: the window is on the right monitor's workspace 2, and \
         both monitors show 1"
    );
    assert!(
        !headed_on_stage(&desk.state, app.pane),
        "the premise: the window is out of sight"
    );

    let _ = relaunched(&mut desk, &app, "floating");
    settled(&mut desk);
    assert_eq!(
        (
            desk.state.panes.id_of(&window),
            says(&desk, &right),
            showing(&desk)
        ),
        (Some(app.pane), "2".to_owned(), "1".to_owned()),
        "(the window's pane, what the right monitor shows, what the left one \
         shows) after the relaunch: the right monitor should have gone to the \
         window's workspace and the left one stayed"
    );
    assert_eq!(
        desk.state.focused_window(),
        Some(window.clone()),
        "the window was brought into view without the keyboard"
    );
}

/// Where `window` is drawn now, and the pane it is in.
fn drawn_in_its_pane(
    desk: &Desk,
    window: &Window,
) -> (crate::pane::PaneId, Rectangle<f64, Logical>) {
    let pane = desk
        .state
        .panes
        .id_of(window)
        .expect("the window is in a pane");
    (
        pane,
        drawn_now(&desk.state, pane, desk.state.clock.now()).rect,
    )
}

/// Whether `outer` holds `inner`, to within half a pixel of
/// an easing's rounding.
fn holds(outer: Rectangle<f64, Logical>, inner: Rectangle<f64, Logical>) -> bool {
    const SLACK: f64 = 0.5;
    outer.loc.x <= inner.loc.x + SLACK
        && outer.loc.y <= inner.loc.y + SLACK
        && outer.loc.x + outer.size.w + SLACK >= inner.loc.x + inner.size.w
        && outer.loc.y + outer.size.h + SLACK >= inner.loc.y + inner.size.h
}

/// **On the workspace in view, in tiling, the window only
/// grows back into its own tile.** The launch's window
/// splits the tile when it opens, as it does for any launch:
/// nothing can know yet that the application will answer with
/// a window it has. From the moment it does, the window is
/// drawn growing back into the tile it had, each frame holding
/// the one before, and it ends exactly there. Before, it left
/// its tile for the launch's and regrew from there: it
/// vanished from its tile and reappeared beside it, about 436
/// ms of motion by SVC-S9's frames. The token comes back 150
/// ms after the launch here, inside the 129 to 163 ms the
/// spike measured, so both windows are still on their way.
#[test]
fn relaunching_an_application_in_tiling_on_the_workspace_in_view_grows_its_window_back_into_its_own_tile()
 {
    let (mut desk, app) = running(Some("super+t"));
    let window = window_of(&desk, app.pane);
    let tile = desk.placed(app.pane);
    let launch = launched(&mut desk);
    assert_ne!(
        desk.placed(app.pane),
        tile,
        "the premise: the launch's window split the application's tile"
    );
    desk.state.clock.advance(Duration::from_millis(150));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    frame(&mut desk);

    let (_, mut last) = drawn_in_its_pane(&desk, &window);
    let token = desk.state.launch_token(launch);
    activates(&mut desk, &app.surface, &token);
    for step in 0..40 {
        let (pane, drawn) = drawn_in_its_pane(&desk, &window);
        assert!(
            holds(drawn, last),
            "frame {step} after the token: the window is drawn at {drawn:?}, \
             which does not hold where it was drawn the frame before, {last:?}: \
             it left its own tile, which is {tile:?}"
        );
        assert_eq!(
            pane, app.pane,
            "frame {step} after the token: the window changed panes"
        );
        last = drawn;
        desk.state.clock.advance(Duration::from_millis(16));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        frame(&mut desk);
    }
    settled(&mut desk);
    let (_, drawn) = drawn_in_its_pane(&desk, &window);
    assert_eq!(
        (desk.placed(app.pane), drawn),
        (tile, tile.to_f64()),
        "(where the layout puts it, where it is drawn) once the relaunch has \
         settled: the window did not end in the tile it had"
    );
}

/// **The control: a window the application opens for the
/// launch is still its arrival.** An application that opens
/// a window on every activation -- or a launcher that forks
/// and exits -- maps a window of its own after the launch
/// began, newer than the launch's, and `claim_into` moves it
/// into the window opened for the launch, as it always has.
/// The window that was already there is left alone, and an
/// arrival is not an activation: the scripts are not told one.
#[test]
fn a_window_opened_for_a_relaunch_still_arrives_in_the_window_opened_for_the_launch() {
    for (key, mode) in MODES {
        let (mut desk, app) = running(key);
        let window = window_of(&desk, app.pane);
        let launch = launched(&mut desk);
        let new = desk.open_surface();
        let opened = window_of(&desk, new.pane);
        assert!(
            launch < new.pane,
            "{mode}: the premise: the new window is newer than the launch"
        );
        let before = told(&desk).len();
        let token = desk.state.launch_token(launch);
        activates(&mut desk, &new.surface, &token);
        settled(&mut desk);
        assert_eq!(
            (
                desk.state.panes.id_of(&opened),
                desk.state.panes.id_of(&window)
            ),
            (Some(launch), Some(app.pane)),
            "{mode}: (the new window's pane, the old window's pane): the window \
             opened for the relaunch did not arrive in the one opened for the \
             launch, or the one that was there was disturbed"
        );
        let after = &told(&desk)[before..];
        assert!(
            !after.iter().any(|event| event.starts_with("activate")),
            "{mode}: an arrival was told as an activation: {after:?}"
        );
    }
}

/// **No `close` reaches the scripts for a window that never
/// closed**, in every mode, with the window on the workspace
/// in view and on another. The launch's window is heard going
/// -- it has -- and the application's is heard as
/// `activate <id> launch`. Before, the scripts heard `close`
/// for the application's window as it was moved, under a new
/// id, into the launch's.
#[test]
fn relaunching_an_application_tells_the_scripts_no_close_for_its_window() {
    for (key, mode) in MODES {
        for away in [false, true] {
            let mode = format!("{mode}, {}", if away { "away" } else { "in view" });
            let (mut desk, app) = running(key);
            if away {
                sent_away(&mut desk, &app, &mode);
            }
            let before = told(&desk).len();
            let launch = relaunched(&mut desk, &app, &mode);
            settled(&mut desk);
            let after = told(&desk)[before..].to_vec();
            let (id, launch) = (app.pane.get(), launch.get());
            assert!(
                !after.contains(&format!("close {id}")),
                "{mode}: the scripts heard the application's window, {id}, \
                 close, and it never did: {after:?}"
            );
            assert!(
                after.contains(&format!("close {launch}")),
                "{mode}: the launch's window, {launch}, went and the scripts \
                 were not told: {after:?}"
            );
            assert!(
                after.contains(&format!("activate {id} launch")),
                "{mode}: the scripts were not told the application's window \
                 was brought forward for the launch: {after:?}"
            );
        }
    }
}

/// **An ordinary activation is told too, as `request`, and
/// the shipped scripts leave the view alone for it.** Nothing
/// checks where a token came from -- any client can make one
/// for itself -- so a view that followed every request would
/// be one any application could pull away. The compositor's
/// own answer is unchanged: a window on a workspace nobody is
/// looking at is refused, and stays out of sight.
#[test]
fn a_genuine_activation_is_told_as_a_request_and_the_view_stays_where_it_is() {
    let (mut desk, app) = running(Some("super+t"));
    sent_away(&mut desk, &app, "tiling");
    let before = told(&desk).len();
    let token = genuine_token(&mut desk);
    activates(&mut desk, &app.surface, &token);
    settled(&mut desk);
    let after = told(&desk)[before..].to_vec();
    assert_eq!(
        (after, showing(&desk), workspace_of(&desk, app.pane)),
        (
            vec![format!("activate {} request", app.pane.get())],
            "1".to_owned(),
            "2".to_owned()
        ),
        "(what the scripts were told, the workspace in view, the window's): a \
         genuine activation was not told as a request, or the view followed it"
    );
}

/// **A window being closed is not brought into view or given
/// the keyboard by a relaunch**, in every mode, on the
/// workspace in view and on another. An application relaunched
/// in the moment its window is fading out brings that window
/// forward with the launch's token; the compositor refuses a
/// window being closed the keyboard whatever the token, and
/// the shipped `activate` handler must not hand it over anyway
/// or carry the view to a window that is going.
#[test]
fn relaunching_an_application_whose_window_is_being_closed_leaves_the_view_and_the_keyboard_where_they_were()
 {
    for (key, mode) in MODES {
        for away in [false, true] {
            let mode = format!("{mode}, {}", if away { "away" } else { "in view" });
            let (mut desk, app) = running(key);
            if away {
                sent_away(&mut desk, &app, &mode);
            }
            let other = desk.open_surface();
            desk.pump();
            desk.pump();
            if desk
                .state
                .panes
                .get(other.pane)
                .and_then(Pane::placed)
                .is_some()
            {
                desk.answer(&other);
            }
            settled(&mut desk);
            let other_window = window_of(&desk, other.pane);
            desk.state.focus_window(
                &other_window,
                SERIAL_COUNTER.next_serial(),
                crate::script::FocusSource::Script,
            );
            desk.state.close_pane(app.pane);
            frame(&mut desk);
            assert!(
                desk.state.panes.get(app.pane).is_some_and(Pane::leaving),
                "{mode}: the premise: the application's window is being closed"
            );
            let before = told(&desk).len();
            let _ = relaunched(&mut desk, &app, &mode);
            frame(&mut desk);
            let after = told(&desk)[before..].to_vec();
            assert!(
                after.contains(&format!("activate {} launch", app.pane.get())),
                "{mode}: the scripts were not told of the relaunch: {after:?}"
            );
            assert_eq!(
                (desk.state.focused_window(), showing(&desk)),
                (Some(other_window.clone()), "1".to_owned()),
                "{mode}: (the window with the keyboard, the workspace in view): a \
                 relaunch gave the keyboard to a window being closed, or carried \
                 the view to it: {after:?}"
            );
        }
    }
}
