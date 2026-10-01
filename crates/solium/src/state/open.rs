//! A window opening: launching a program and the loading stand-in shown until it arrives, giving
//! a new client its pane or the one opened for its launch, placing and animating it the first
//! time it has something to show, whether it takes the keyboard then (`first_focus`), and the
//! scripts' `open` event.

use super::*;

/// A process and the processes that started it, up to a few generations.
///
/// The program the compositor spawns is not always the one that connects: a
/// flatpak, a shell wrapper or a launcher forks and the client is a
/// grandchild. Walking up from the client finds the launch that started it
/// anyway. Bounded because this runs when a window appears and `/proc` is not
/// free, and because a chain longer than this is not a launch we started.
fn ancestry(pid: u32) -> Vec<u32> {
    const GENERATIONS: usize = 8;
    let mut family = Vec::with_capacity(GENERATIONS);
    let mut current = pid;
    for _ in 0..GENERATIONS {
        family.push(current);
        // Field 4 of /proc/<pid>/stat is the parent. The command name in
        // field 2 may contain spaces and parentheses, so the tail is taken
        // from the last ')' rather than by splitting the whole line.
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{current}/stat")) else {
            break;
        };
        let Some(tail) = stat.rsplit_once(')') else {
            break;
        };
        let Some(parent) = tail
            .1
            .split_whitespace()
            .nth(1)
            .and_then(|field| field.parse::<u32>().ok())
        else {
            break;
        };
        if parent <= 1 {
            break;
        }
        current = parent;
    }
    family
}

/// What the scripts did with a window opening, as [`Solium::trigger_open`]
/// reports it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Opened {
    /// A script answered with commands of its own, so the built-in open
    /// animation is not wanted.
    handled: bool,
    /// A script asked for the keyboard to go somewhere -- a `sol.focus`
    /// among its commands. Where a new window's keyboard goes is then the
    /// script's decision and not [`Solium::offer_keyboard`]'s: see
    /// `a_script_that_moves_the_keyboard_at_open_has_the_last_word`.
    focused: bool,
}

/// What kind of client a window is. Named for [`first_focus`], which gives
/// both kinds the same answer, and on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClientKind {
    /// An `xdg_toplevel`.
    Xdg,
    /// A managed XWayland window: Steam, a Wine game, anything X11.
    X11,
}

impl ClientKind {
    fn of(window: &Window) -> Option<Self> {
        Self::from_roles(window.toplevel().is_some(), window.x11_surface().is_some())
    }

    /// The kind a window with these roles is, apart from [`Self::of`] so it
    /// can be asked about an X11 window, which a test cannot make. `None` for a
    /// window with neither role, which smithay's `Window` cannot be today.
    /// `a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is`.
    pub(super) const fn from_roles(xdg: bool, x11: bool) -> Option<Self> {
        if xdg {
            Some(Self::Xdg)
        } else if x11 {
            Some(Self::X11)
        } else {
            None
        }
    }
}

/// How a window is given the keyboard as it is first shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FirstFocus {
    /// Through [`Solium::focus_window`], as a click or `sol.focus` would be:
    /// raised, handed the keyboard through the gate, activated in X11's own
    /// terms, and announced to the scripts as `focus`.
    Focus,
    /// Not at all: the keyboard stays where it was.
    Stay,
}

/// The rule [`Solium::offer_keyboard`] applies, apart so that it can be asked
/// about an X11 window, which a test cannot make: an `X11Surface` needs a live
/// XWayland.
///
/// **Both kinds, and the same answer for both.** Until #134's review an X11
/// window got the keyboard as it opened only because `scrolling.lua` focused
/// every window but a dialog that opened, whether or not it was in charge.
/// Nobody had decided that; `new_toplevel`, which gave an xdg window the
/// keyboard, never sees an X11 one. When that stray call was stopped, Steam and
/// every Wine game opened with the keyboard left on the window before. Both kinds are named in the match so
/// that leaving one out again is an edit to this function and to
/// `a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is`, rather than
/// a line somewhere that forgets.
///
/// Only a managed window: an unmanaged one is a menu, a tooltip or a splash,
/// placed by its client (`xwayland.rs`'s `places_itself`). And only one headed
/// somewhere the user can see, which is [`Solium::on_stage`].
///
/// **The kind is an `Option`, so that the whole decision is here.**
/// `offer_keyboard` used to return early when [`ClientKind::of`] answered
/// nothing, and in #134's first round that early return was the line --
/// `toplevel().is_none()` -- that left every X11 window without the keyboard.
/// A kind nobody knows is `Stay` here instead, and `offer_keyboard` asks this
/// once with no return of its own:
/// `a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is` pins the
/// rule, [`ClientKind::from_roles`] and that shape.
pub(super) const fn first_focus(
    kind: Option<ClientKind>,
    managed: bool,
    on_stage: bool,
) -> FirstFocus {
    match kind {
        Some(ClientKind::Xdg | ClientKind::X11) => {
            if managed && on_stage {
                FirstFocus::Focus
            } else {
                FirstFocus::Stay
            }
        }
        None => FirstFocus::Stay,
    }
}

impl Solium {
    /// Give a newly mapped client a pane, and answer with its id.
    ///
    /// Where a window enters the compositor. `sync_panes` would notice it at
    /// the next refresh anyway; doing it here is what makes the id exist for
    /// the script that is about to be told the window opened.
    pub(crate) fn take_pane(&mut self, window: Window) -> u64 {
        let slot = self.real_geometry(&window).unwrap_or_default();
        self.panes.mapped(window, slot, self.clock.now()).get()
    }

    /// A pane for a surface that places itself: a menu, a tooltip, a drag icon.
    ///
    /// Unmanaged, so no layout ever sees it, and bare, so it never grows a
    /// titlebar. Both matter: without the first, dragging a text selection out
    /// of an application reflows the whole desktop to make room for the drag
    /// icon; without the second, a tooltip gets a title bar.
    pub(crate) fn take_unmanaged_pane(&mut self, window: Window) {
        let slot = self.real_geometry(&window).unwrap_or_default();
        let id = self.panes.mapped(window, slot, self.clock.now());
        if let Some(pane) = self.panes.get_mut(id) {
            pane.unmanage();
        }
        self.decorations.set_bare(&mut self.panes, id);
    }

    /// Which process a client belongs to, as the kernel reports it.
    ///
    /// The compositor's own view of who is on the other end of the socket, not
    /// anything the client said about itself.
    fn client_pid(&self, window: &Window) -> Option<u32> {
        let surface = window.wl_surface()?;
        let client = surface.client()?;
        let credentials = client.get_credentials(&self.display_handle).ok()?;
        u32::try_from(credentials.pid).ok()
    }

    /// Move a client into the window that was opened for its launch.
    ///
    /// The late half of adoption. `new_toplevel` matches on the process and
    /// gets it right for anything that stays as the process we spawned; a
    /// program whose launcher forks and exits breaks that chain and opens a
    /// window of its own. When it then activates with the token we gave it,
    /// this puts it where it belonged: the window it was already in is retired
    /// and its content moves to the one that has been waiting.
    ///
    /// Returns whether the token was this window's own -- it is in the window
    /// the token was minted for, now or already -- so that only a token that
    /// is not falls through to being an ordinary request for focus.
    ///
    /// **Already** is the ordinary case, and it is asked first. An application
    /// that is itself the process `sol.spawn` started was adopted by its pid
    /// in `new_toplevel`, so by the time it activates with the token it was
    /// handed -- GTK, Qt and winit all do, alacritty among them -- its window
    /// is no longer loading. Asked after the loading test, as it was, this
    /// answer was never reached for any such window: the token fell through,
    /// `request_activation` focused the window wherever it was, and with
    /// `follow_overflow = false` that was a workspace nobody is looking at
    /// (#134 review). The keyboard for a launched window is
    /// [`Self::offer_keyboard`]'s to decide, at its first frame, and a token
    /// that only says "this is the window you opened for me" is not a second
    /// opinion. Nor a brief one: `request_activation` now takes the keyboard
    /// back off a window nobody can see, but a window focused on the way has
    /// still been told it had the keyboard and the clipboard, and the scripts
    /// that it was focused. `a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_by_its_own_token`
    /// sends the token before the first frame, as winit does, and after, and
    /// asserts both; `a_launched_window_on_screen_takes_the_keyboard_when_it_activates_with_its_own_token`
    /// is the other half.
    pub(super) fn claim_into(&mut self, pane: crate::pane::PaneId, surface: &WlSurface) -> bool {
        let Some(window) = self.window_for(surface) else {
            return false;
        };
        let Some(wrong) = self.panes.id_of(&window) else {
            return false;
        };
        if wrong == pane {
            return true;
        }
        // Still waiting, or already given up on.
        if !self.panes.get(pane).is_some_and(Pane::is_loading) {
            return false;
        }

        if let Some(held) = self.panes.get_mut(pane) {
            held.adopt(window.clone());
        }
        // The pane it opened in goes, and with it the frame and the id nothing
        // should have learned. Retired rather than left empty: `sync_panes`
        // would drop it anyway, and the layout is told now rather than a frame
        // late.
        //
        // **At once, with no fade, and removed before it is told** -- the one
        // way a window leaves that does not go through `Self::depart` (#126).
        // It is a merge, not a close: the client it held is still on screen,
        // in the pane above, and a fade here would draw a second copy of it
        // leaving. Removed first because for this one line both panes hold
        // the same `Window`, and `close`'s snapshot would list it twice.
        self.panes.remove(wrong);
        self.trigger_close(wrong);
        tracing::debug!(
            pane = pane.get(),
            was = wrong.get(),
            "an application arrived in its window, by token"
        );
        // **The window it arrived in may be one nobody can see**, and the
        // window that moved into it may already have the keyboard: it opened on
        // screen, as a window of its own, and was offered it there. The
        // keyboard does not follow it onto a hidden workspace; `hand_off_keyboard`
        // does nothing for a window that does not have it.
        // `a_focused_window_claimed_into_a_pane_on_a_hidden_workspace_gives_the_keyboard_up`.
        if !self.pane_on_stage(pane) {
            self.hand_off_keyboard(&window);
        }
        self.redraw = true;
        true
    }

    /// Give a mapped client to the window that was opened for it, or open a
    /// new one.
    ///
    /// The whole point of the refactor arrives here. A client whose process is
    /// the one a window has been waiting for becomes that window's content:
    /// same id, same slot, same frame with the same animation still running in
    /// it. Nothing is created and nothing is replaced, so nothing downstream
    /// ever learns that the window used to be empty.
    ///
    /// Everything that can go wrong ends in an ordinary window. A client that
    /// re-execs or forks past the ancestor walk, one whose window was closed
    /// while it was still starting, one nobody asked for — each of them opens
    /// the old way. A missed adoption is a window that appears normally; it is
    /// never a window that is lost.
    ///
    /// Must happen where the window is mapped rather than later: `sync_panes`
    /// gives any client it finds without a pane one of its own, and by then
    /// there would be two windows for one application.
    pub(super) fn adopt_or_open(&mut self, window: Window) -> u64 {
        let waiting = match self.client_pid(&window) {
            Some(pid) => self.panes.awaiting(&ancestry(pid)),
            None => None,
        };
        let Some(id) = waiting else {
            return self.take_pane(window);
        };
        let Some(pane) = self.panes.get_mut(id) else {
            return self.take_pane(window);
        };
        pane.adopt(window);
        let slot = pane.slot();
        tracing::debug!(pane = id.get(), "an application arrived in its window");

        // Told its size straight away, rather than on its first commit. A
        // client that learns its size only after it has drawn paints one frame
        // at a size it chose for itself, and that frame is visible -- so the
        // window that has been standing there at the right size all along
        // flickers to the wrong one and back at the exact moment it fills.
        if let Some(window) = self.panes.get(id).and_then(Pane::client).cloned() {
            size_window(&window, slot);
            self.map_stacked(window, slot.loc, false);
        }
        id.get()
    }

    /// Start a program as a client of this compositor, from
    /// [`crate::launch::command`]: with the environment Solium was started
    /// with, and the session's own variables on top.
    /// `launch::tests::a_spawned_program_gets_the_environment_solium_started_with`.
    pub(crate) fn spawn(&mut self, program: &str, args: &[String]) {
        use std::process::Stdio;

        let mut process = crate::launch::command(program);
        process
            .args(args)
            // Without this the child inherits the *host* display and opens its
            // window next to the compositor rather than inside it.
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Errors are inherited, not discarded. A program that refuses to
            // start says why on stderr, and swallowing that leaves "the
            // binding does nothing" as the only symptom of a dozen different
            // causes.
            .stderr(Stdio::inherit());

        // Our own X server if we have one, and emphatically not the host's if
        // we do not. Inheriting DISPLAY is worse than it sounds: a Qt or GTK
        // program prefers X11 when it is set, connects to the *host's*
        // XWayland, and opens its window on the host desktop. The spawn logs
        // success, nothing errors, and no window ever appears here.
        match self.x11_display {
            Some(number) => process.env("DISPLAY", format!(":{number}")),
            None => process.env_remove("DISPLAY"),
        };
        // The desktop systemd and D-Bus activation were told, `Lilium` when
        // the session file named none, rather than none at all.
        // `session::tests::the_desktop_is_lilium_unless_the_session_said_otherwise`.
        process.env(
            "XDG_CURRENT_DESKTOP",
            crate::session::desktop(std::env::var("XDG_CURRENT_DESKTOP").ok()),
        );
        if std::env::var_os("XDG_SESSION_TYPE").is_none() {
            process.env("XDG_SESSION_TYPE", "wayland");
        }

        // Before the fork, so the window is on screen and the other windows
        // have moved aside by the time the program has been asked to start.
        let pane = self.begin_loading(program, None);

        // And a token in the child's environment, naming the window we just
        // opened for it.
        //
        // This is what makes the window find its application whatever the
        // application does to its own processes. Matching on the pid works
        // right up until a launcher forks and exits -- Firefox does -- and then
        // the chain from the client runs into init and stops. A token does not
        // care: we made it, we handed it over, and whatever comes back holding
        // it is the thing we launched.
        let token = self.launch_token(pane);
        process.env("XDG_ACTIVATION_TOKEN", &token);
        // The older spelling, for programs that only look for that one.
        process.env("DESKTOP_STARTUP_ID", &token);

        match process.spawn() {
            Ok(mut child) => {
                tracing::info!(program, socket = self.socket_name, "spawned");
                // And it learns whose process to wait for here, because there
                // was no process to name a moment ago.
                if let Some(pane) = self.panes.get_mut(pane) {
                    pane.expect(child.id());
                }
                // Waited on so the child is reaped — a compositor that leaves
                // zombies eventually cannot fork at all — and so that an early
                // exit is *reported*. A program that starts and immediately
                // quits is indistinguishable from one that never drew, and
                // that is the hard version of this to debug.
                let name = program.to_owned();
                std::thread::spawn(move || match child.wait() {
                    Ok(status) if status.success() => {
                        tracing::debug!(program = name, "a spawned program exited cleanly");
                    }
                    Ok(status) => {
                        tracing::warn!(program = name, %status, "a spawned program exited");
                    }
                    Err(err) => tracing::warn!(program = name, ?err, "could not wait for a child"),
                });
            }
            Err(err) => {
                // Nothing is coming, so the window goes now rather than
                // sitting there for the whole of `patience` promising
                // otherwise. The layout is told, and closes the gap, while the
                // window fades out of it. See `Self::depart`.
                self.depart(pane);
                self.redraw = true;
                tracing::warn!(?err, program, "could not spawn");
            }
        }
    }

    /// An activation token naming the window opened for a launch, for the
    /// launched program's environment. Apart from [`Self::spawn`] so that a
    /// test can hand its own client the token a launch would have.
    pub(super) fn launch_token(&mut self, pane: crate::pane::PaneId) -> String {
        let data = XdgActivationTokenData::default();
        data.user_data.insert_if_missing(|| LaunchedFor(pane));
        let (token, _) = self.activation_state.create_external_token(data);
        token.as_str().to_owned()
    }

    /// Put a stand-in on screen for an application that was just asked for.
    ///
    /// At the pointer, because that is where the asking happened and where the
    /// eye already is. It is not where the window will end up -- the layout
    /// decides that when the window exists -- but the window grows out of this
    /// rect when it arrives, so the movement is continuous either way.
    /// Open a window for an application that has been asked for.
    ///
    /// The window's life begins here rather than when the client connects: it
    /// takes a slot, the other windows move aside for it, and it can be closed
    /// while it waits. What arrives later maps *into* it.
    ///
    /// Its first slot is under the pointer, because that is where the asking
    /// happened and where the eye already is. The layout is told immediately
    /// and usually moves it somewhere better in the same breath — which is the
    /// point: the arrangement settles before the application has done
    /// anything at all.
    pub(crate) fn begin_loading(&mut self, program: &str, pid: Option<u32>) -> crate::pane::PaneId {
        let name = std::path::Path::new(program)
            .file_name()
            .map_or(program, |name| name.to_str().unwrap_or(program));
        // Resolved now, so reloading the configuration mid-wait does not
        // change what a window already on screen looks like halfway through.
        let source = crate::pane::loading_source(self.loading.scene.as_deref());
        // Built here rather than at the first draw: the scene is what the
        // window *is* until its application arrives, and a window that is
        // empty for its first frame is a window that flickers.
        let properties = format!(
            "{{\"program\":\"{}\",\"waited\":0}}",
            name.replace('"', "'")
        );
        let scene = match crate::surface::ShellSurface::new(source.clone(), &properties) {
            Ok(scene) => Some(scene),
            Err(err) => {
                // The window still opens. It takes its slot, it can be closed,
                // and its application will still arrive in it -- it just has
                // nothing to show meanwhile, which beats not opening.
                tracing::warn!(
                    ?err,
                    program = name,
                    "no scene for a window that is loading"
                );
                None
            }
        };
        self.open_loading(name, pid, source, scene)
    }

    /// The rest of [`Self::begin_loading`]: the window itself, once its scene
    /// has been built, or has failed to be and is `None`.
    ///
    /// Apart so that a test can open a window for an application without
    /// starting Qt, which a test process holding a raw libwayland connection
    /// does not survive -- see
    /// `changed_output_resends_fractional_scale_and_unchanged_output_does_not`.
    /// Everything a layout hears and does about the window is on this side of
    /// the cut, and `None` is the path a failed scene already takes.
    pub(super) fn open_loading(
        &mut self,
        name: &str,
        pid: Option<u32>,
        source: std::path::PathBuf,
        scene: Option<crate::surface::ShellSurface>,
    ) -> crate::pane::PaneId {
        // A window's worth of screen from the very first frame, before anyone
        // is asked where it should go. A layout usually moves it in the same
        // breath, but this is what it falls back to -- and the fallback has to
        // be the shape of the window that is coming, because for a floating
        // arrangement this *is* where the window ends up. Getting this wrong
        // is not subtle: the application arrives sized to whatever is here.
        let area = self.launch_slot();
        let id = self.panes.open(Pane::loading(
            name,
            pid,
            area,
            source,
            scene,
            self.clock.now(),
        ));

        // Built whether or not it will be drawn yet. The room it takes is
        // reserved from the first frame, so the window is the same shape
        // before and after its application arrives -- and it is the *same*
        // frame, keyed by pane, so whatever animation is running in it carries
        // straight through the handover instead of starting again.
        self.decorations
            .insert(&mut self.panes, id, area.size.w, area.size.h);
        // And the frame's share comes off the slot, exactly as it does for a
        // window the layout placed, so the client is sized to the same rect
        // either way.
        let client = inner(area, self.insets_of(id));
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_slot(client);
        }
        tracing::debug!(program = name, ?pid, "a window opened for an application");

        // Told as an *open*, not as a relayout. A layout keeps its own
        // arrangement and adds to it when it hears a window opened; a relayout
        // only re-runs what it already holds, so the new window would never
        // join. This is the whole of "the other windows move aside": the
        // window opened, and it opened before its application existed.
        //
        // Unless it was asked not to. `reserves_a_slot` has to gate the event
        // and not only the snapshot: a layout that has been told a window
        // opened keeps it in its own arrangement, and would go on placing it
        // however the snapshot were filtered afterwards.
        if self.loading.reserves_a_slot {
            self.trigger_open(id);
        }
        self.redraw = true;
        id
    }

    /// Give up on applications that never arrived.
    ///
    /// A window that waits forever holds a slot forever. It goes exactly as if
    /// it had been closed, and the layout is told — so the arrangement heals
    /// rather than keeping a gap for something that is not coming.
    ///
    /// Returns whether anything went, so the backend redraws.
    pub(crate) fn settle_loading(&mut self, now: std::time::Duration) -> bool {
        let patience = self.loading.patience;
        let gone: Vec<(crate::pane::PaneId, String)> = self
            .panes
            .iter()
            // `is_loading` as well, because `expired` answers for a pane whose
            // client has gone too, and that one is `settle_leaving`'s to drop.
            // (`depart` would do nothing with it -- it has gone already -- but
            // the log line below would call it an application that never came.)
            .filter(|pane| pane.is_loading() && pane.expired(now, patience))
            .map(|pane| (pane.id(), pane.program().unwrap_or_default().to_owned()))
            .collect();
        if gone.is_empty() {
            return false;
        }
        for (id, program) in gone {
            tracing::info!(program, "gave up on an application that never arrived");
            // The way every window goes: the layout is told while the pane is
            // still here, and it fades out of its place as the layout closes
            // up -- the scene that stood in for the application fading as a
            // closed window does. This used to remove the pane first so that
            // "a layout should not be laying out around a window that is
            // already gone", which `close`'s own snapshot now answers for every
            // route alike: the window is listed there as leaving, and in no
            // event after it. See `Self::depart`.
            self.depart(id);
        }
        self.redraw = true;
        true
    }

    /// Where a window goes when nothing else has an opinion about it.
    ///
    /// A window's worth of screen, inset from the work area. Used for a window
    /// opened for an application when no layout placed it — floating, or no
    /// scripts at all — so that what is on screen while the application starts
    /// is the shape and size of the window that is coming.
    fn launch_slot(&self) -> Rectangle<i32, Logical> {
        let area = self
            .work_area()
            .unwrap_or_else(|| Rectangle::new((0, 0).into(), (1280, 800).into()));
        let inset = 48;
        Rectangle::new(
            (area.loc.x + inset, area.loc.y + inset).into(),
            (
                (area.size.w - inset * 2).max(200),
                (area.size.h - inset * 2).max(150),
            )
                .into(),
        )
    }

    /// Place and animate a window the first time it has something to show.
    ///
    /// Both belong to this moment rather than to the map request: until the
    /// client has committed a buffer it has no size, and placing or animating
    /// a zero-sized window is placing nothing.
    pub(super) fn show_if_new(&mut self, window: &Window) {
        // A client's first commit is typically empty — it commits to receive
        // the initial configure, then draws. Claiming the first-show moment on
        // that commit places and animates a zero-sized window, which lands it
        // at half the output away from where it belongs.
        let size = window.geometry().size;
        if size.w <= 0 || size.h <= 0 {
            return;
        }

        let Some(pane) = self.panes.id_of(window) else {
            return;
        };
        if !self.panes.get(pane).is_some_and(present::mark_shown) {
            return;
        }

        // An unmanaged pane is already where it belongs, and everything below
        // this line sizes, places or animates -- all three wrong for it.
        //
        // Override-redirect is X11 for "do not manage me": a menu, a tooltip,
        // a drag icon. `mapped_override_redirect_window` has already mapped it
        // at the position its client chose, and `take_unmanaged_pane` marked
        // the pane so. Marking it shown above is still right -- it is on
        // screen -- but it is the last thing this function may do to it.
        //
        // Issue #100, from the reporter's log opening a Steam context menu:
        //
        //     WARN could not size an X11 window err=UnsupportedForOverrideRedirect
        //
        // That warning is the harmless half. `size_window` asks smithay to
        // configure an override-redirect surface and is refused, which costs a
        // line in the log and nothing else. The damage is the next statement:
        // `initial_placement` picks a location and `map_element` *succeeds* at
        // moving the menu there, so it opens away from the pointer and the
        // layout treats it as a window.
        //
        // Hence the guard here and not inside `size_window`: the failing call
        // is not the one doing the harm, and a guard there would have silenced
        // the warning while leaving the menu misplaced.
        //
        // This is the second leak of its kind -- see the comment in
        // `xwayland.rs`'s `mapped_override_redirect_window`, where unmanaged
        // windows reached the list a layout reads and dragging a text
        // selection reflowed the desktop. Both were one path forgetting to
        // ask; if a third appears, the question belongs inside whatever those
        // paths call rather than at a fourth call site.
        //
        // Asked here, ahead of both branches below -- one places from a
        // remembered slot, the other from a fresh fit -- rather than inside
        // whichever branch a bug happened to surface in.
        if !self.panes.get(pane).is_some_and(Pane::managed) {
            return;
        }

        // A client that never negotiates still gets a frame.
        //
        // `Frame::Pending` reserves `TITLEBAR_HEIGHT` -- see `insets_for` --
        // on the understanding that a frame is on its way. Until this, the
        // only things that ever built one were `decorate`, driven by
        // `xdg_decoration`, and two special cases (a launch placeholder and
        // leaving fullscreen). So a client that never binds that protocol
        // reserved a titlebar for the life of its window and had none drawn.
        //
        // That is not a rare case. Firefox does not bind it, and no XWayland
        // client can -- Steam included. Issue #103, measured nested: exactly
        // 32 rows of unpainted space above Firefox's first painted row, beside
        // a real titlebar on a terminal in the same session.
        //
        // Server-side is already what `new_decoration` offers unasked, on the
        // grounds that the frame is part of the desktop's look. This extends
        // that to the clients that never ask: having no opinion gets the same
        // answer as not having expressed one yet. A client that later asks for
        // client-side is still obeyed -- `decorate` takes it to `Frame::None`
        // and removes this.
        //
        // `Pending` is the whole condition and it is exact: a negotiated
        // server-side frame is already `Styled`, a negotiated client-side one
        // is already `None`, and `insert` itself returns early on `Styled`.
        // What is left is only "nobody has decided", which is this.
        //
        // Ahead of the sizing below, because a frame changes the insets that
        // `fitted_size` and `initial_placement` both read.
        if self
            .panes
            .get(pane)
            .is_some_and(|pane| matches!(pane.frame(), crate::pane::Frame::Pending))
        {
            let real = self.real_geometry(window);
            let width = real.map_or(TITLEBAR_HEIGHT * 20, |real| real.size.w);
            let height = real.map_or(TITLEBAR_HEIGHT * 15, |real| real.size.h);
            self.decorations
                .insert(&mut self.panes, pane, width, height);
        }

        // A client that mapped into a window which was already open takes
        // that window's shape. The layout placed it before the application
        // existed and was told it opened then; doing either again would move a
        // window that is already where it belongs and announce it twice.
        if self.panes.get(pane).is_some_and(Pane::adopted) {
            let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
                return;
            };
            // Sized again: adoption already asked for this, and a client that
            // negotiated its decorations in between has a different amount of
            // room than it was first told.
            size_window(window, slot);
            self.map_stacked(window.clone(), slot.loc, false);
            // Its place was decided at the launch, so the keyboard is decided
            // now: nothing was asked of a script on the way here.
            self.offer_keyboard(window, pane);
            return;
        }

        // Sized to fit before it is placed, because where a window goes
        // depends on how big it is.
        let size = self.fitted_size(window);
        let location = self.initial_placement(window, size);
        if size != window.geometry().size {
            size_window(window, Rectangle::new(location, size));
        }
        self.map_stacked(window.clone(), location, true);

        // How a window appears is a script's decision — that is what makes the
        // dock-icon genie a script rather than a feature. The built-in is only
        // a fallback for when nothing has an opinion; a window popping into
        // existence with no animation at all is worse than a plain one.
        let opened = self.trigger_open(pane);
        if !opened.handled
            && let Some(outer) = self.outer_geometry(window)
            && let Some(pane) = self.panes.get(pane)
        {
            present::open(pane, outer, self.clock.now());
        }
        // After `open`, which is where the layout says where the window goes,
        // and only if no script said where the keyboard goes.
        if !opened.focused {
            self.offer_keyboard(window, pane);
        }
    }

    /// Give a window the keyboard as it is first shown, if it is headed
    /// somewhere the user can see.
    ///
    /// Focus follows the newest window; #12 turns this into a policy. It used
    /// to happen in `new_toplevel`, which is too early to know where the window
    /// is going: a window opened by its application is told to the layout only
    /// here, at its first frame, so every new toplevel took the keyboard before
    /// anything had placed it -- and one a layout then parked on a workspace
    /// nobody is looking at kept it. A window launched with `sol.spawn` was
    /// placed before its application existed, and took the keyboard when the
    /// application arrived, wherever that was. With `follow_overflow = false`
    /// both are ordinary, and every key typed afterwards went to a window the
    /// user could not see (#134 review). So the grant waits for the window to
    /// have its place, and asks [`Self::on_stage`] -- the question the focus
    /// rules ask everywhere else -- first. Declined, the keyboard is left where
    /// it was. The two routes are
    /// `a_window_that_overflows_to_a_hidden_workspace_does_not_take_the_keyboard`
    /// and
    /// `a_launched_window_parked_on_a_hidden_workspace_does_not_take_the_keyboard_when_it_arrives`;
    /// a launched window that is on screen still takes it, in
    /// `a_launched_window_that_overflows_with_the_view_takes_the_keyboard_when_it_arrives`.
    ///
    /// **An xdg window and an X11 one alike, and through
    /// [`Self::focus_window`]**: the rule is [`first_focus`], which says why
    /// X11 windows are named in it. Through `focus_window` rather than a bare
    /// [`Self::give_keyboard`], because the grant is only part of focus. The
    /// scripts are told `focus`, as they are for every other way the keyboard
    /// moves, and X11 is told in its own terms (`xwayland::activate`, whose
    /// note says what an X11 window does when it is not). Before the second
    /// round of #134's review a new xdg window in floating or tiling got the
    /// bare grant and no `focus`, while in scrolling it got all of it through
    /// the strip's own `sol.focus`; the three modes now take one path, which
    /// `a_window_opening_while_floating_takes_the_keyboard_as_a_focus` and its
    /// `tiling` and `scrolling` twins pin.
    ///
    /// Through the gate, which refuses it while locked: this is a client
    /// opening a window of its own accord, with nobody at the machine, and
    /// until the gate existed it was the shortest way to the password.
    /// `focus_window` asks the gate before it does anything, so a refused
    /// window is not raised or activated either
    /// (`a_window_that_opens_while_locked_does_not_take_the_keyboard`).
    fn offer_keyboard(&mut self, window: &Window, pane: crate::pane::PaneId) {
        let landed = self.settling();
        let screens = self.screens();
        let offer = self.panes.get(pane).map_or(FirstFocus::Stay, |held| {
            first_focus(
                ClientKind::of(window),
                held.managed(),
                self.on_stage(held, &screens, landed),
            )
        });
        match offer {
            FirstFocus::Focus => self.focus_window(window, SERIAL_COUNTER.next_serial()),
            FirstFocus::Stay => tracing::debug!(
                pane = pane.get(),
                "a window opened where nobody can see it, and the keyboard stayed put"
            ),
        }
    }

    pub(super) fn trigger_open(&mut self, pane: crate::pane::PaneId) -> Opened {
        let id = pane.get();
        // The size limits this open is about to tell the layouts, in the
        // window's row, recorded as told: a change after this is measured from
        // what the open carried (#115). For a window its application opened,
        // the commit that brought them has recorded them already, and the
        // `open` is the only thing the layouts hear. See
        // `Solium::notice_limits`, and
        // `real_client::client_sizes::a_window_that_opens_with_a_minimum_hears_it_once`.
        if let Some(held) = self.panes.get_mut(pane)
            && let Some(limits) = held.client().map(crate::state::limits_of)
        {
            held.set_limits(limits);
        }
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return Opened::default();
        };
        let outcome = scripts.opened(id, snapshot);
        self.scripts = Some(scripts);

        let opened = Opened {
            handled: outcome.handled && !outcome.commands.is_empty(),
            focused: outcome
                .commands
                .iter()
                .any(|command| matches!(command, Command::Focus { .. })),
        };
        self.apply(outcome);
        opened
    }

    /// Where a new window goes.
    ///
    /// Centred, then cascaded, so a second window is not hidden exactly behind
    /// the first. This is *not* a layout engine and is not trying to be one —
    /// E4 replaces it with floating, tiling and scrolling behind one interface.
    /// It exists because "every window at (0, 0)" is not a usable compositor.
    /// The size a window should open at, which is not always the one it asked
    /// for.
    ///
    /// A client picks its own first size and plenty pick one larger than the
    /// screen — Firefox and LibreOffice both do on a 1600x900 output. Nothing
    /// was bringing it down, and placement cannot help: a window wider than the
    /// display hangs off it wherever you put it. Firefox opened with its tab
    /// bar visible and everything below the fold past the bottom edge.
    ///
    /// A layout that claims the window overrides this a moment later. This is
    /// for the floating case, where nothing else has an opinion.
    fn fitted_size(&self, window: &Window) -> Size<i32, Logical> {
        let size = window.geometry().size;
        // A window that already has a place is measured against its own
        // monitor; a brand new one against the active one, which is where it
        // is about to be put.
        let area = self
            .real_geometry(window)
            .and_then(|real| self.work_area_of(real))
            .or_else(|| self.work_area());
        let Some(area) = area else {
            return size;
        };
        let insets = self.frame_insets(window);
        (
            size.w.min((area.size.w - insets.horizontal()).max(1)),
            size.h.min((area.size.h - insets.vertical()).max(1)),
        )
            .into()
    }

    fn initial_placement(&self, window: &Window, size: Size<i32, Logical>) -> Point<i32, Logical> {
        let Some(output) = self.work_area() else {
            return (0, 0).into();
        };

        const CASCADE: i32 = 44;
        const WRAP: usize = 6;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "the index is taken modulo a small constant"
        )]
        let step = CASCADE * (self.panes.len() % WRAP) as i32;

        // The frame is above the client, so the client's own top edge starts
        // that far down: the pair has to fit in the work area, not just the
        // client.
        let insets = self.frame_insets(window);
        let outer_height = size.h + insets.vertical();
        let outer_width = size.w + insets.horizontal();

        let centred = |available: i32, window: i32| (available - window) / 2;
        let x = output.loc.x + centred(output.size.w, outer_width).max(0) + step + insets.left;
        let y = output.loc.y + centred(output.size.h, outer_height).max(0) + step + insets.top;

        // Kept on the output even if the cascade would walk a large window off
        // the bottom right.
        (
            x.min(output.loc.x + (output.size.w - size.w).max(0)),
            y.max(output.loc.y + insets.top)
                .min(output.loc.y + (output.size.h - outer_height).max(0) + insets.top),
        )
            .into()
    }
}
