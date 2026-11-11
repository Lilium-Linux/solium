//! **The keyboard belongs to the lock screen, whatever the session
//! behind it does.**
//!
//! One test per route by which the keyboard used to reach an
//! application while the session was locked, each driven the way it
//! happens: a real application client and a real lock client on two
//! sockets, each with a `wl_keyboard` of its own, keys pressed through
//! the same filter the backends feed. Every route ends on the same
//! four measures, taken from both ends of the wire: the server's
//! keyboard is on a lock surface, the application's client was never
//! told it has the keyboard, it was never made the clipboard's client,
//! and a key typed reaches the lock client and not the application.
//! Then each one unlocks and checks that the keyboard comes back,
//! because a fix that sealed the keyboard for good would pass
//! everything before that.
//!
//! Each of those route tests fails against `69835bc`, the stage they
//! were fixed on. That was checked by putting that commit's code back
//! under them and watching each one fail -- a security test that
//! passes either way is worse than none.
//!
//! Then the tests of **who holds the lock**, from `the lock's holder`
//! on: a second lock client, an application asking for a lock of its
//! own, an unlock from a lock that was never granted, the lock client
//! crashing, a mode active at the lock, a lock surface dying, the key
//! held at unlock, a capture while locked and a menu closing. Each of
//! those drives real clients too, with a third one where the attack or
//! the second lock screen needs one, and each fails against
//! `2b6550e`; the doc of each says where.
//!
//! Then the two where #127's close meets the rule, which neither branch
//! could write alone: `a_close_in_flight_across_a_lock_never_takes_the_keyboard`
//! and `a_close_with_a_menu_open_takes_the_keyboard_off_the_menu`. Each
//! fails against the merge of the two with its own fix taken out; the
//! doc of each says where.
//!
//! The X11 test is the exception to all of them: see its doc.
//!
//! Then the tests of **when `locked` is sent**, from
//! `locked_is_not_sent_in_the_dispatch_that_asked_for_it` on: `lock.rs`
//! told the lock client `locked` in the dispatch that asked for the
//! lock, while every monitor was still showing the desktop. Each fails
//! against this branch with `lock` confirming at once again, as it did
//! before -- except the first half of
//! `with_no_monitor_locked_goes_out_at_once`, which is the one case where
//! "at once" was already right; the doc of each says where. Every lock
//! in this module now also has its frames shown (`Session::show`),
//! because without them `locked` does not come.

mod layer_keyboard;
use super::*;
use smithay::backend::input::KeyState;
use smithay::input::keyboard::Keycode;

/// One client's end of the wire.
struct Side {
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    qh: QueueHandle<Client>,
    client: Client,
}

impl Side {
    /// A client with a keyboard and a data device, bound the way
    /// any application binds them.
    fn connect(display: &mut Display<Solium>, state: &mut Solium) -> Self {
        let (conn, queue, client) = connect(display, state);
        let qh = queue.handle();
        let mut side = Self {
            conn,
            queue,
            qh,
            client,
        };
        let seat = side.client.seat.clone().expect("wl_seat bound");
        side.client
            .data_devices
            .clone()
            .expect("wl_data_device_manager bound")
            .get_data_device(&seat, &side.qh, ());
        // The keyboard is not asked for here: the seat's handler
        // takes the one keyboard a client has, and a second would
        // be sent every key again. That costs a second round trip,
        // one to hear the seat's capabilities and one for the
        // server to see the `get_keyboard` sent in answer --
        // `a_mode_active_at_the_lock_does_not_garble_the_password`
        // counts the keys that arrive.
        side.pump(display, state);
        side.pump(display, state);
        assert!(
            side.client.keyboard.is_some(),
            "the seat offered a keyboard and the client took it"
        );
        side
    }

    fn pump(&mut self, display: &mut Display<Solium>, state: &mut Solium) {
        pump(
            display,
            state,
            &self.conn,
            &self.qh,
            &mut self.queue,
            &mut self.client,
        );
    }

    /// A round trip for a client the server may disconnect on the
    /// way, which `pump` would read as the fixture failing.
    /// Returns the protocol error it was disconnected with, if any.
    fn pump_or_error(
        &mut self,
        display: &mut Display<Solium>,
        state: &mut Solium,
    ) -> Option<wayland_client::backend::protocol::ProtocolError> {
        self.conn.display().sync(&self.qh, ());
        // A client already disconnected cannot flush; what it was
        // disconnected with is still read below.
        let _flushed = self.conn.flush();
        display
            .dispatch_clients(state)
            .expect("dispatching the round trip");
        display
            .flush_clients()
            .expect("flushing the server's events");
        match self.queue.blocking_dispatch(&mut self.client) {
            Ok(_) => None,
            Err(_) => self.conn.protocol_error(),
        }
    }

    /// Ask for a lock, and if `cover`, a lock surface for every
    /// monitor, the way a lock screen does. Nothing is asserted:
    /// this is also how an application behind the lock asks.
    fn ask_lock(
        &mut self,
        display: &mut Display<Solium>,
        state: &mut Solium,
        cover: bool,
    ) -> (ext_session_lock_v1::ExtSessionLockV1, Vec<LockSurfaceProxy>) {
        let locks = self
            .client
            .locks
            .clone()
            .expect("ext_session_lock_manager_v1 bound");
        let compositor = self.client.compositor.clone().expect("wl_compositor bound");
        let lock = locks.lock(&self.qh, ());
        let mut surfaces = Vec::new();
        if cover {
            for output in self.client.outputs.clone() {
                let surface = compositor.create_surface(&self.qh, ());
                let role = lock.get_lock_surface(&surface, &output, &self.qh, ());
                surfaces.push(LockSurfaceProxy { surface, role });
            }
        }
        self.pump(display, state);
        (lock, surfaces)
    }
}

/// One lock surface, from the client's side.
struct LockSurfaceProxy {
    surface: wl_surface::WlSurface,
    role: ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
}

impl LockSurfaceProxy {
    /// Destroy it the way `swaylock` does when its monitor goes.
    fn destroy(&self) {
        self.role.destroy();
        self.surface.destroy();
    }
}

impl Side {
    /// Open a window, and run the frame that follows it.
    ///
    /// `sync_panes` is what the backends call once a frame, and it
    /// is where a window arriving or going is noticed -- and where
    /// `settle_focus` is called from. A test that skipped it would
    /// skip one of the routes.
    fn open(
        &mut self,
        display: &mut Display<Solium>,
        state: &mut Solium,
    ) -> (
        Window,
        xdg_toplevel::XdgToplevel,
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
    ) {
        let opened = open_xdg(display, state, &self.conn, &self.client, &self.qh);
        state.sync_panes();
        self.pump(display, state);
        opened
    }

    /// Open a menu on `parent` that asks for a grab, as a context
    /// menu does: the grab before the first commit, as the
    /// protocol requires, with the serial of the keyboard's last
    /// enter.
    fn menu(
        &mut self,
        display: &mut Display<Solium>,
        state: &mut Solium,
        parent: &xdg_surface::XdgSurface,
    ) -> xdg_popup::XdgPopup {
        let compositor = self.client.compositor.clone().expect("wl_compositor bound");
        let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
        let seat = self.client.seat.clone().expect("wl_seat bound");

        let surface = compositor.create_surface(&self.qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &self.qh, ());
        let positioner = wm_base.create_positioner(&self.qh, ());
        positioner.set_size(32, 32);
        positioner.set_anchor_rect(0, 0, 1, 1);
        let popup = xdg.get_popup(Some(parent), &positioner, &self.qh, ());
        popup.grab(&seat, self.client.serial);
        surface.commit();
        self.pump(display, state);
        popup
    }
}

/// One monitor, an application client and a lock client that has
/// not locked anything yet.
struct Session {
    display: Display<Solium>,
    state: Solium,
    app: Side,
    locker: Side,
    /// The lock client's surfaces from its last `lock`, one per
    /// monitor.
    lock_surfaces: Vec<LockSurfaceProxy>,
    /// Every monitor with its `wl_output` global, so one can be
    /// unplugged the way a backend unplugs it.
    monitors: Vec<(
        Output,
        smithay::reexports::wayland_server::backend::GlobalId,
    )>,
}

impl Session {
    fn new() -> Self {
        Self::with_monitors(1)
    }

    /// `count` monitors side by side, each 1920x1080.
    fn with_monitors(count: i32) -> Self {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        // See the #99 test: a Qt scene in a process holding a
        // libwayland connection of its own aborts the test binary.
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));

        let mut monitors = Vec::new();
        for index in 0..count {
            // A global, which the other fixtures' monitors are
            // not: a lock surface is asked for per `wl_output`, so
            // the lock client has to be able to name one. Before
            // either client connects, so it is in both registries.
            monitors.push(Self::monitor(&display, &mut state, index));
        }

        let app = Side::connect(&mut display, &mut state);
        let locker = Side::connect(&mut display, &mut state);
        Self {
            display,
            state,
            app,
            locker,
            lock_surfaces: Vec::new(),
            monitors,
        }
    }

    /// The `index`th monitor, 1920x1080 and to the right of the
    /// one before it, with its global.
    fn monitor(
        display: &Display<Solium>,
        state: &mut Solium,
        index: i32,
    ) -> (
        Output,
        smithay::reexports::wayland_server::backend::GlobalId,
    ) {
        let output = Output::new(
            format!("lock-focus-test-{index}"),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_string(),
                model: "lock-focus".to_string(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        let global = output.create_global::<Solium>(&display.handle());
        state.space.map_output(&output, (1920 * index, 0));
        (output, global)
    }

    /// The `index`th monitor.
    fn output(&self, index: usize) -> Output {
        self.monitors
            .get(index)
            .map(|(output, _)| output.clone())
            .expect("a monitor this fixture made")
    }

    /// A monitor plugged in, the way both backends add one: a
    /// global, mapped, and `settle_monitors`.
    fn plug(&mut self) -> Output {
        let index = i32::try_from(self.monitors.len()).expect("a handful of monitors");
        let monitor = Self::monitor(&self.display, &mut self.state, index);
        let output = monitor.0.clone();
        self.monitors.push(monitor);
        self.state.settle_monitors();
        output
    }

    /// A monitor unplugged -- or switched off by the configuration,
    /// which `tty.rs` drops the same way -- as `drop_screen` and then
    /// `resync_screens` do it: the global removed, the layers closed,
    /// the output unmapped, and `settle_monitors`.
    fn unplug(&mut self, index: usize) {
        let (output, global) = self.monitors.remove(index);
        self.display.handle().remove_global::<Solium>(global);
        crate::layer::close_all(&output);
        self.state.space.unmap_output(&output);
        self.state.settle_monitors();
    }

    /// `output` shows a frame built now: the backend's half of
    /// "When `locked` is sent" in `lock.rs`, played the only way it
    /// can be without a renderer -- `lock_frame` as the frame is
    /// built, `frame_presented` when it is on the screen.
    fn show(&mut self, output: &Output) {
        let frame = self.state.lock_frame();
        self.state.frame_presented(output, frame);
    }

    /// Every monitor shows a frame built now.
    fn show_all(&mut self) {
        for (output, _) in self.monitors.clone() {
            self.show(&output);
        }
    }

    /// Whether the lock client has been told `locked` on `lock`,
    /// after a round trip for it to hear anything that was sent.
    fn told_locked(&mut self, lock: &ext_session_lock_v1::ExtSessionLockV1) -> bool {
        self.locker.pump(&mut self.display, &mut self.state);
        self.locker
            .client
            .locked
            .contains(&wayland_client::Proxy::id(lock))
    }

    /// Whether the server's keyboard is on one of the lock
    /// client's surfaces.
    ///
    /// Asked of the lock itself and not of anything in `focus.rs`:
    /// a test that checked the gate by asking the gate would pass
    /// whatever the gate did.
    fn keyboard_on_lock(&self) -> bool {
        let Some(focus) = self
            .state
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
        else {
            return false;
        };
        self.state
            .lock
            .as_ref()
            .is_some_and(|lock| lock.surfaces().any(|each| each.wl_surface() == &focus))
    }

    /// Lock the session the way a lock screen does: lock, then a
    /// surface for the monitor. Then every monitor shows the lock,
    /// which is what `locked` waits for.
    fn lock(&mut self) -> ext_session_lock_v1::ExtSessionLockV1 {
        let (lock, surfaces) = self
            .locker
            .ask_lock(&mut self.display, &mut self.state, true);
        self.lock_surfaces = surfaces;
        self.show_all();
        self.locker.pump(&mut self.display, &mut self.state);
        self.app.pump(&mut self.display, &mut self.state);

        assert!(
            self.state.lock.is_some(),
            "the lock client asked and the session did not lock, so \
             nothing after this is about a locked session"
        );
        assert!(
            self.locker
                .client
                .locked
                .contains(&wayland_client::Proxy::id(&lock)),
            "the lock client was not told `locked`"
        );
        assert!(
            self.keyboard_on_lock(),
            "the lock surface mapped and the keyboard is not on it: \
             `focus_lock` is the one hand-off the gate must let through"
        );
        assert!(
            self.locker.client.keyboard_on.is_some(),
            "and the lock client was told so"
        );
        assert!(
            self.app.client.keyboard_on.is_none(),
            "locking did not take the keyboard away from the application"
        );
        lock
    }

    /// Type one key, and say who heard it: the application, and
    /// the lock client.
    fn type_key(&mut self) -> (bool, bool) {
        self.app.client.keys.clear();
        self.locker.client.keys.clear();
        // `a`, which is evdev 30 and 38 to xkb.
        crate::input::key(&mut self.state, Keycode::new(38), KeyState::Pressed, 1);
        crate::input::key(&mut self.state, Keycode::new(38), KeyState::Released, 2);
        self.app.pump(&mut self.display, &mut self.state);
        self.locker.pump(&mut self.display, &mut self.state);
        (
            !self.app.client.keys.is_empty(),
            !self.locker.client.keys.is_empty(),
        )
    }

    /// What every route ends on: the keyboard is still the lock
    /// screen's by every measure there is.
    fn assert_sealed(&mut self, route: &str, selections_before: usize) {
        assert!(
            self.keyboard_on_lock(),
            "{route}: the keyboard left the lock screen"
        );
        assert!(
            self.app.client.keyboard_on.is_none(),
            "{route}: the application's client was told it has the keyboard"
        );
        assert!(
            self.locker.client.keyboard_on.is_some(),
            "{route}: the lock client was not told it has the keyboard"
        );
        assert_eq!(
            self.app.client.selections, selections_before,
            "{route}: the application behind the lock was made the \
             clipboard's client"
        );
        let (app, locker) = self.type_key();
        assert!(
            !app,
            "{route}: a key typed at the lock screen reached the \
             application behind it, and that key is a password"
        );
        assert!(
            locker,
            "{route}: the key reached nobody at all, so the line \
             above proves nothing"
        );
    }

    /// Unlock, and the keyboard comes back to a window without the
    /// mouse being touched.
    fn assert_unlocks(&mut self, lock: ext_session_lock_v1::ExtSessionLockV1) {
        lock.unlock_and_destroy();
        self.locker.pump(&mut self.display, &mut self.state);
        self.app.pump(&mut self.display, &mut self.state);

        assert!(
            self.state.lock.is_none(),
            "the lock client unlocked and the session did not"
        );
        let focus = self
            .state
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        assert!(
            focus
                .as_ref()
                .is_some_and(|surface| self.state.window_for(surface).is_some()),
            "the session unlocked with the keyboard on no window. \
             `unlock` clears `lock` before `settle_focus` so that the \
             gate lets it through; a session that ignores typing until \
             you move the mouse reads as one that did not unlock"
        );
        assert!(
            self.app.client.keyboard_on.is_some(),
            "and the application was not told it has it back"
        );
        let (app, _) = self.type_key();
        assert!(app, "unlocked, and typing does not reach the application");
    }
}

/// **A window opening while locked.** `new_toplevel` set the
/// keyboard itself, straight onto the new window: an application
/// that opened a window behind the lock took the password.
#[test]
fn a_window_that_opens_while_locked_does_not_take_the_keyboard() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    assert!(
        session.app.client.keyboard_on.is_some(),
        "the application has the keyboard before the lock, or \
         taking it away proves nothing"
    );
    let lock = session.lock();
    let selections = session.app.client.selections;

    session.app.open(&mut session.display, &mut session.state);

    session.assert_sealed("a window opened while locked", selections);
    session.assert_unlocks(lock);
}

/// **#49: a lock part of the way through a glide leaves no window
/// transformed**: the glide lands behind the lock as anywhere
/// else, the window is a plain element at the monitor's rectangle
/// once it has, and it is still there at rest when the lock lifts.
#[test]
fn a_lock_mid_glide_leaves_the_window_at_rest() {
    use super::fullscreen_glides::{SCRIPT, before, between, screen, script_at};

    let mut session = Session::new();
    session
        .state
        .start_scripts(Some(script_at("locked-mid-glide", SCRIPT)));
    let (window, _toplevel, surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    commit_buffer(&session.app.client, &session.app.qh, &surface, 400, 300);
    session.app.pump(&mut session.display, &mut session.state);
    session
        .state
        .space
        .map_element(window.clone(), (300, 200), false);
    session.state.space.refresh();
    session.state.sync_panes();
    let pane = session.state.panes.id_of(&window).expect("a pane");
    session.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let land = |session: &mut Session, after: Duration| {
        session.state.clock.advance(after);
        let now = session.state.clock.now();
        session.state.settle(now);
        session.state.sync_panes();
    };
    let drawn = |session: &Session| {
        let held = session.state.panes.get(pane).expect("the pane");
        session
            .state
            .drawn_at(
                held,
                session.state.pane_outer(held),
                session.state.clock.now(),
            )
            .rect
    };
    let transformed = |session: &Session| {
        session
            .state
            .panes
            .get(pane)
            .is_some_and(present::transformed)
    };
    land(&mut session, Duration::from_secs(1));

    assert!(session.state.trigger("super+f"));
    session.app.pump(&mut session.display, &mut session.state);
    session.state.clock.advance(Duration::from_millis(100));
    assert!(
        between(drawn(&session), before(), screen()),
        "the premise: part of the way into fullscreen"
    );
    let lock = session.lock();
    commit_buffer(&session.app.client, &session.app.qh, &surface, 1920, 1080);
    session.app.pump(&mut session.display, &mut session.state);
    land(&mut session, Duration::from_secs(1));
    assert!(!transformed(&session), "landed behind the lock, at rest");
    assert_eq!(drawn(&session), screen(), "covering the monitor");

    lock.unlock_and_destroy();
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(session.state.lock.is_none(), "the premise: unlocked");
    land(&mut session, Duration::from_millis(16));
    assert!(!transformed(&session), "and at rest once the lock lifts");
    assert_eq!(drawn(&session), screen());
}

/// **What is pressed at the lock screen is not told to the
/// configuration.** A Caps Lock toggled there, with `us,ru` and
/// Russian live, reaches no `keyboard` listener, so no pill policy
/// runs behind the lock and nothing it showed is waiting when the lock
/// lifts; and the unlock does not tell it late. A toggle after the
/// unlock is told as ever. The lock screen draws its own Caps Lock
/// warning, if it wants one.
#[test]
fn a_caps_toggle_at_the_lock_screen_is_not_told_to_the_configuration() {
    use crate::keyboard_change::tests::{RECORDER, configured, seen};
    use crate::keymap::tests::{CAPS, set_us_ru, tap};

    let mut session = Session::new();
    set_us_ru(&mut session.state, 1);
    configured(&mut session.state, "locked", RECORDER);
    session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();

    tap(&mut session.state, &[CAPS]);
    assert!(session.state.keyboard.caps, "Caps Lock went on at the lock");
    assert_eq!(
        seen(&session.state),
        "",
        "and the configuration was not told"
    );

    session.assert_unlocks(lock);
    assert_eq!(seen(&session.state), "", "nor told once the lock lifted");
    tap(&mut session.state, &[CAPS]);
    assert_eq!(
        seen(&session.state),
        "caps:2:Russian:RU:false:false",
        "a toggle after the unlock is told"
    );
}

/// **A window asking to be brought forward while locked.** A token
/// says the request came from something the user was using, not
/// that anyone is at the machine now. `request_activation` focuses
/// through `focus_window`, which asks the gate first, and #134's
/// second review added a step after that: a window off every screen
/// has the keyboard handed on from it. Asked of a window on screen
/// and of one off every screen, so that both branches run behind the
/// lock and neither leaves it.
#[test]
fn a_window_asking_to_be_brought_forward_while_locked_does_not_take_the_keyboard() {
    let mut session = Session::new();
    let (window, _, surface, _) = session.app.open(&mut session.display, &mut session.state);
    let pane = session
        .state
        .panes
        .id_of(&window)
        .expect("an open window has a pane");
    let lock = session.lock();
    let selections = session.app.client.selections;

    for off_screen in [false, true] {
        if off_screen {
            // Placed as a layout places, so that where it is going
            // is off every screen and not only where it is.
            let now = session.state.clock.now();
            session.state.place(
                pane.get(),
                to_rect(Rectangle::new((5000, 5000).into(), (400, 300).into())),
                AnimationSpec::default(),
                now,
                Standing::Free,
            );
        }
        assert_eq!(
            session.state.pane_on_stage(pane),
            !off_screen,
            "the premise: the window is where this case needs it"
        );
        let token = {
            let (token, _) = session
                .state
                .activation_state
                .create_external_token(XdgActivationTokenData::default());
            token.as_str().to_owned()
        };
        session
            .app
            .client
            .activation
            .clone()
            .expect("xdg_activation_v1 bound")
            .activate(token, &surface);
        session.app.pump(&mut session.display, &mut session.state);
        session.assert_sealed(
            &format!(
                "a window {} asked to be brought forward while locked",
                if off_screen {
                    "off every screen"
                } else {
                    "on screen"
                }
            ),
            selections,
        );
    }
    // Back on screen, so that unlocking has a window to give the
    // keyboard to.
    let now = session.state.clock.now();
    session.state.place(
        pane.get(),
        to_rect(Rectangle::new((100, 100).into(), (400, 300).into())),
        AnimationSpec::default(),
        now,
        Standing::Free,
    );
    session.assert_unlocks(lock);
}

/// **A window closing while locked.** `sync_panes` calls
/// `settle_focus`, which returns early only if a *window* has
/// focus. A lock surface is not a window, so it went on to its
/// topmost arm and gave the keyboard to the top application.
#[test]
fn a_window_that_closes_while_locked_does_not_take_the_keyboard() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let (_window, toplevel, surface, xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();
    let selections = session.app.client.selections;

    toplevel.destroy();
    xdg.destroy();
    surface.destroy();
    session.app.pump(&mut session.display, &mut session.state);
    // The frame after it: the space lets go of the dead window and
    // `sync_panes` sees the set of windows change.
    session.state.space.refresh();
    assert!(
        session.state.sync_panes(),
        "closing a window did not change the set of windows, so \
         `settle_focus` never ran and this proves nothing"
    );

    session.assert_sealed("a window closed while locked", selections);
    session.assert_unlocks(lock);
}

/// **A menu opened while locked.** `grab` set the keyboard itself,
/// onto the menu, and installed a keyboard grab that ignores every
/// later attempt to move it.
#[test]
fn a_menu_that_grabs_while_locked_does_not_take_the_keyboard() {
    let mut session = Session::new();
    let (_window, _toplevel, _surface, parent) =
        session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();
    let selections = session.app.client.selections;

    let _menu = session
        .app
        .menu(&mut session.display, &mut session.state, &parent);

    assert!(
        !session
            .state
            .seat
            .get_keyboard()
            .expect("the seat has a keyboard")
            .is_grabbed(),
        "a menu behind the lock was given a keyboard grab"
    );
    assert!(
        session.app.client.popups_done > 0,
        "the grab was refused and the menu was not told, so it waits \
         on a grab that is not coming and is still open after unlock"
    );
    session.assert_sealed("a menu grabbed while locked", selections);
    session.assert_unlocks(lock);
}

/// **A menu already open when the session locks.** Its keyboard
/// grab ignored `lock`'s attempt to take the keyboard away and
/// `focus_lock`'s attempt to give it to the lock screen, then
/// re-aimed the keyboard at the menu on every key.
#[test]
fn a_menu_open_when_the_session_locks_gives_the_keyboard_up() {
    let mut session = Session::new();
    let (_window, _toplevel, _surface, parent) =
        session.app.open(&mut session.display, &mut session.state);
    let _menu = session
        .app
        .menu(&mut session.display, &mut session.state, &parent);
    let keyboard = session
        .state
        .seat
        .get_keyboard()
        .expect("the seat has a keyboard");
    assert!(
        keyboard.is_grabbed(),
        "the menu asked for a grab and did not get one, so locking \
         over it proves nothing"
    );

    let lock = session.lock();
    assert!(
        !keyboard.is_grabbed(),
        "the menu's keyboard grab outlived the lock"
    );
    assert!(
        session.app.client.popups_done > 0,
        "the menu open at the lock was not dismissed, so its chain \
         outlives the lock"
    );
    let selections = session.app.client.selections;
    session.assert_sealed("a menu was open when the session locked", selections);
    session.assert_unlocks(lock);

    // The chain really was ended, not just ungrabbed. Left recorded
    // on the seat, this second menu would be told it is not the
    // topmost popup -- a protocol error, which kills the client and
    // fails the round trip inside `menu`.
    let _again = session
        .app
        .menu(&mut session.display, &mut session.state, &parent);
    assert!(
        keyboard.is_grabbed(),
        "after unlock, a menu could not grab: the chain from before \
         the lock is still in the way"
    );
}

/// **`focus_window`, called while locked**, as a script's
/// `sol.focus` calls it, and as a layout's event handler, a
/// focus-follows-view workspace and xdg-activation all do -- none
/// of which stop running when the session locks.
#[test]
fn focusing_a_window_while_locked_does_not_move_the_keyboard() {
    let mut session = Session::new();
    let (window, ..) = session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();
    let selections = session.app.client.selections;

    session.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    session.app.pump(&mut session.display, &mut session.state);

    session.assert_sealed("focus_window while locked", selections);
    session.assert_unlocks(lock);
}

/// **`sol.act("windows.focus")` behind the lock is answered
/// `locked`**: `focus_window` refuses it, so `done` is not told it
/// was done, and the keyboard stays the lock screen's. Tested with
/// the Cyrillic group active (#132).
#[test]
fn sol_act_focus_behind_the_lock_is_answered_locked() {
    let mut session = Session::new();
    russian(&mut session.state);
    let _ = session.app.open(&mut session.display, &mut session.state);
    let id = session
        .state
        .snapshot()
        .windows
        .first()
        .map(|window| window.id)
        .expect("a window");
    let lock = session.lock();
    let selections = session.app.client.selections;
    let directory = std::env::temp_dir().join(format!("solium-locked-act-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            r#"sol.act("windows.focus", {{ id = {id} }}, function(ok, reason) sol.status(tostring(ok) .. " " .. tostring(reason)) end)"#
        ),
    )
    .expect("writing the entry point");
    session.state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));
    let _ = std::fs::remove_dir_all(&directory);
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(session.state.status, "false locked");
    session.assert_sealed("sol.act windows.focus while locked", selections);
    session.assert_unlocks(lock);
}

/// **`sol.act("windows.minimize")` behind the lock is answered
/// `locked`**, and so is `"windows.unminimize"`, as a focus is: the
/// window is left as it was (#30). Tested with the Cyrillic group
/// active (#132).
#[test]
fn sol_act_minimize_behind_the_lock_is_answered_locked() {
    let mut session = Session::new();
    russian(&mut session.state);
    let _ = session.app.open(&mut session.display, &mut session.state);
    let id = session
        .state
        .snapshot()
        .windows
        .first()
        .map(|window| window.id)
        .expect("a window");
    let lock = session.lock();
    let directory =
        std::env::temp_dir().join(format!("solium-locked-minimize-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            r#"answers = ""
local function told(ok, reason) answers = answers .. tostring(ok) .. " " .. tostring(reason) .. ";"; sol.status(answers) end
sol.act("windows.minimize", {{ id = {id} }}, told)
sol.act("windows.unminimize", {{ id = {id} }}, told)"#
        ),
    )
    .expect("writing the entry point");
    session.state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));
    let _ = std::fs::remove_dir_all(&directory);
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(session.state.status, "false locked;false locked;");
    assert!(
        session
            .state
            .snapshot()
            .windows
            .iter()
            .all(|window| !window.minimized),
        "a window was minimized behind the lock"
    );
    session.assert_unlocks(lock);
}

/// **Focusing a minimized window behind the lock leaves it
/// minimized**: the focus is refused, and the window is not shown
/// and nothing is reflowed for it, as `sol.act("windows.unminimize")`
/// is answered `locked` (#30).
#[test]
fn focusing_a_minimized_window_behind_the_lock_leaves_it_minimized() {
    let mut session = Session::new();
    russian(&mut session.state);
    // Another window, for the keyboard to be on when the session
    // unlocks.
    let _ = session.app.open(&mut session.display, &mut session.state);
    let (window, ..) = session.app.open(&mut session.display, &mut session.state);
    let pane = session.state.panes.id_of(&window).expect("a pane");
    session.state.minimize(pane);
    let lock = session.lock();
    session.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Activate,
    );
    assert!(
        session.state.panes.get(pane).is_some_and(Pane::minimized),
        "a focus behind the lock showed a minimized window"
    );
    session.assert_unlocks(lock);
}

/// **The last line: a key cannot reach a surface the gate would
/// have refused, even when something has got the keyboard there.**
///
/// Done here the only way it can be, by calling the one method
/// the gate exists to own -- which stands for the route nobody has
/// found yet.
#[test]
fn a_key_does_not_reach_an_application_that_got_past_the_gate() {
    let mut session = Session::new();
    let (window, ..) = session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();

    let surface = window
        .wl_surface()
        .expect("the window has a surface")
        .into_owned();
    let keyboard = session
        .state
        .seat
        .get_keyboard()
        .expect("the seat has a keyboard");
    #[expect(
        clippy::disallowed_methods,
        reason = "going round the gate on purpose: this is the test of what holds when something does"
    )]
    keyboard.set_focus(
        &mut session.state,
        Some(surface),
        SERIAL_COUNTER.next_serial(),
    );
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.app.client.keyboard_on.is_some(),
        "the keyboard was not moved onto the application, so this \
         proves nothing"
    );

    let (app, _) = session.type_key();
    assert!(
        !app,
        "a key typed while locked reached an application that held \
         the keyboard: nothing at delivery checks the rule"
    );
    session.assert_unlocks(lock);
}

/// **The lock's holder: a second lock is refused, and the first
/// keeps working.** `swayidle`'s `before-sleep` starting a second
/// `swaylock` while the first is up. The second lock was granted
/// and *replaced* the first: the real lock screen's surfaces were
/// dropped and the keyboard taken off them, and the newcomer --
/// which, told `finished`, would have exited -- was left the only
/// lock. The user was locked out of their own session.
///
/// Against `2b6550e`, fails at "can no longer be typed into".
#[test]
fn a_second_lock_is_refused_and_the_first_keeps_working() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();
    let selections = session.app.client.selections;

    let mut second = Side::connect(&mut session.display, &mut session.state);
    let (again, _) = second.ask_lock(&mut session.display, &mut session.state, false);
    let again_id = wayland_client::Proxy::id(&again);
    let refused =
        second.client.finished.contains(&again_id) && !second.client.locked.contains(&again_id);
    // What `swaylock` does with `finished`: destroy the lock, and
    // exit.
    again.destroy();
    let _gone = second.pump_or_error(&mut session.display, &mut session.state);
    drop(second);
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);

    let (app, locker) = session.type_key();
    assert!(
        locker,
        "after a second lock was asked for, the real lock screen can \
         no longer be typed into: the user is locked out of their own \
         session"
    );
    assert!(!app, "and the key reached the application");
    assert!(
        refused,
        "a second lock was granted while the lock client holding the \
         session was still there"
    );
    session.assert_sealed("a second lock was asked for", selections);
    session.assert_unlocks(lock);
}

/// **An application cannot put up a lock screen of its own.** It
/// asked for a lock -- which replaced the real one -- and a lock
/// surface on its own `wl_output`, which smithay's duplicate-output
/// check lets through because it compares one client's resources.
/// The gate saw a surface in `Lock::surfaces` and gave it the
/// keyboard: an application drawing a lock screen, and the
/// password typed into it.
///
/// Against `2b6550e`, fails at "was given the keyboard".
#[test]
fn an_application_cannot_put_up_a_lock_screen_of_its_own() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();
    let selections = session.app.client.selections;

    let (theirs, fake) = session
        .app
        .ask_lock(&mut session.display, &mut session.state, true);
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    assert!(
        !fake.is_empty(),
        "the application asked for no lock surface, so this proves nothing"
    );
    assert!(
        session.app.client.keyboard_on.is_none(),
        "an application's own lock surface was given the keyboard: a \
         lock screen that is not the lock screen, and the password goes \
         into it"
    );
    assert!(
        session
            .app
            .client
            .finished
            .contains(&wayland_client::Proxy::id(&theirs)),
        "the application's lock was not refused"
    );
    session.assert_sealed("an application put up a lock screen of its own", selections);
    session.assert_unlocks(lock);
}

/// **Nor in the moment before the real one maps.** The session is
/// locked the instant the lock client asks, before it has drawn
/// anything -- and a monitor plugged in while locked is another
/// monitor the real lock screen has not covered yet.
///
/// Against `2b6550e`, fails at "before the lock screen mapped".
#[test]
fn an_application_cannot_put_up_a_lock_screen_before_the_real_one_maps() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let (lock, _) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, false);
    assert!(
        session.state.lock.is_some(),
        "the lock client asked and the session did not lock, so this \
         proves nothing"
    );

    let (_theirs, _fake) = session
        .app
        .ask_lock(&mut session.display, &mut session.state, true);
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    assert!(
        session.app.client.keyboard_on.is_none(),
        "in the moment before the lock screen mapped, an application's \
         own lock surface was given the keyboard"
    );

    // The real lock screen arrives.
    let compositor = session
        .locker
        .client
        .compositor
        .clone()
        .expect("wl_compositor bound");
    let output = session
        .locker
        .client
        .output
        .clone()
        .expect("wl_output bound");
    let surface = compositor.create_surface(&session.locker.qh, ());
    let role = lock.get_lock_surface(&surface, &output, &session.locker.qh, ());
    session.lock_surfaces = vec![LockSurfaceProxy { surface, role }];
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    // And is on the monitor, so the lock client is told `locked`,
    // without which its unlock below would be refused
    // (`an_unlock_before_locked_unlocks_nothing`).
    session.show_all();
    assert!(
        session.told_locked(&lock),
        "the lock screen is on every monitor and was not told `locked`"
    );

    let selections = session.app.client.selections;
    session.assert_sealed(
        "an application's lock surface came before the real one",
        selections,
    );
    session.assert_unlocks(lock);
}

/// **An unlock from a lock that was never granted unlocks
/// nothing.** `unlock` is not told which lock asked, and smithay
/// 0.7 calls it for `unlock_and_destroy` on any lock object: it
/// posts `invalid_unlock` on one that was never told `locked`, and
/// then, with no `return` after the error, unlocks anyway. So any
/// client could lock and then unlock -- two requests.
///
/// Against `2b6550e`, fails at "in two requests". Against a fix
/// that only refused the second lock, it fails at the same line,
/// through smithay's missing `return`.
#[test]
fn an_unlock_from_a_lock_that_was_never_granted_unlocks_nothing() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();
    let selections = session.app.client.selections;

    let mut intruder = Side::connect(&mut session.display, &mut session.state);
    let (theirs, _) = intruder.ask_lock(&mut session.display, &mut session.state, false);
    theirs.unlock_and_destroy();
    let error = intruder.pump_or_error(&mut session.display, &mut session.state);

    assert!(
        session.state.lock.is_some(),
        "a client that does not hold the lock unlocked the session in \
         two requests"
    );
    let error = error.expect(
        "the intruder was not disconnected: an unlock on a lock that \
         was never told `locked` is `invalid_unlock`",
    );
    // The code only. `unlock_and_destroy` is a destructor, so by
    // the time the error arrives the client has already forgotten
    // the object it is about and cannot say which interface it was.
    assert_eq!(
        error.code,
        ext_session_lock_v1::Error::InvalidUnlock as u32,
        "disconnected, but not for `invalid_unlock`: {error:?}"
    );
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    session.assert_sealed("an unlock from a lock that was never granted", selections);
    // And the holder's own unlock is untouched by all of it.
    session.assert_unlocks(lock);
}

/// **The lock client crashing leaves the session locked**, and a
/// new lock client can take over. The protocol says a lock client
/// dying must not unlock the session. What holds the lock then is
/// nobody, and that must not turn into anybody: a lock refused
/// while the holder lived stays refused after it has died.
///
/// Against `2b6550e`, fails at "was granted": the rival's lock
/// simply replaced the real one, and its unlock then worked
/// whether the real lock client crashed or not.
#[test]
fn the_lock_client_crashing_leaves_the_session_locked() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let _crashing = session.lock();
    let selections = session.app.client.selections;

    let mut rival = Side::connect(&mut session.display, &mut session.state);
    let (refused, _) = rival.ask_lock(&mut session.display, &mut session.state, false);
    assert!(
        rival
            .client
            .finished
            .contains(&wayland_client::Proxy::id(&refused)),
        "a second lock was granted while the lock client holding the \
         session was still there"
    );

    // The lock client crashes: its connection closes with the
    // session still locked. Its replacement is connected first,
    // the way one is started from another terminal.
    let recovery = Side::connect(&mut session.display, &mut session.state);
    let crashed = std::mem::replace(&mut session.locker, recovery);
    session.lock_surfaces.clear();
    drop(crashed);
    session.app.pump(&mut session.display, &mut session.state);
    session
        .locker
        .pump(&mut session.display, &mut session.state);

    assert!(
        session.state.lock.is_some(),
        "the lock client crashed and the session unlocked"
    );
    assert!(
        session.app.client.keyboard_on.is_none(),
        "the lock client crashed and the application was given the keyboard"
    );
    let (app, _) = session.type_key();
    assert!(
        !app,
        "the lock client crashed and a key reached the application"
    );

    refused.unlock_and_destroy();
    let _gone = rival.pump_or_error(&mut session.display, &mut session.state);
    assert!(
        session.state.lock.is_some(),
        "with the lock client dead, a lock refused while it lived \
         unlocked the session"
    );

    // Recovered: a new lock client takes over, and unlocks.
    let lock = session.lock();
    session.assert_sealed("a new lock client took over from a crashed one", selections);
    session.assert_unlocks(lock);
}

/// **A mode active when the session locks does not garble the
/// password.** Presses reached the lock screen, but the filter
/// intercepted every release while a mode was active, so the lock
/// client auto-repeated each key it was never told came up -- and
/// with bindings off while locked, the mode could not be left.
///
/// Against `2b6550e`, fails at "never told it came up".
#[test]
fn a_mode_active_at_the_lock_does_not_garble_the_password() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    // A mode owns input, the way a script's overview or resize
    // mode takes it.
    session.state.script_grab = true;
    let lock = session.lock();

    session.locker.client.key_events.clear();
    session.app.client.keys.clear();
    // `a`, which is evdev 30 and 38 to xkb.
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Pressed, 1);
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Released, 2);
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(
        session.locker.client.key_events,
        vec![(30, true), (30, false)],
        "the lock screen was told a key went down and never told it \
         came up, so it repeats it: a mode active at the lock garbles \
         every key of the password"
    );
    assert!(
        session.app.client.keys.is_empty(),
        "and the key reached the application"
    );

    // The mode ends; the rest is the ordinary unlock.
    session.state.script_grab = false;
    session.assert_unlocks(lock);
}

/// **A lock surface that dies hands the keyboard to one that did
/// not.** A monitor unplugged or a laptop undocked while locked:
/// the lock client destroys that monitor's surface when its output
/// goes. If the keyboard was on it, nothing moved it -- `focus_lock`
/// runs only when a surface maps, and `settle_focus` refused every
/// window without offering the lock screen instead -- so keys went
/// nowhere until another surface arrived.
///
/// Against `2b6550e`, fails at "went nowhere".
#[test]
fn a_lock_surface_that_dies_hands_the_keyboard_to_one_that_did_not() {
    let mut session = Session::with_monitors(2);
    session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();
    assert_eq!(
        session.lock_surfaces.len(),
        2,
        "not a lock surface on each of two monitors, so this proves nothing"
    );

    let focused = session
        .locker
        .client
        .keyboard_on
        .clone()
        .expect("the lock screen has the keyboard");
    let (gone, left): (Vec<_>, Vec<_>) = std::mem::take(&mut session.lock_surfaces)
        .into_iter()
        .partition(|each| wayland_client::Proxy::id(&each.surface) == focused);
    let gone = gone
        .into_iter()
        .next()
        .expect("the keyboard is on one of the lock screen's surfaces");
    gone.destroy();
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);

    let survivor = left
        .first()
        .map(|each| wayland_client::Proxy::id(&each.surface));
    assert_eq!(
        session.locker.client.keyboard_on, survivor,
        "the lock surface the keyboard was on went away and the \
         keyboard went nowhere: typing at the lock screen does nothing \
         until another surface maps"
    );
    session.lock_surfaces = left;
    let selections = session.app.client.selections;
    session.assert_sealed("the focused lock surface died", selections);
    session.assert_unlocks(lock);
}

/// **The key that unlocks is not handed to the window.** The Enter
/// that submits the password is still held when the lock client
/// unlocks, and the keyboard went straight back to the window,
/// whose `wl_keyboard.enter` then said Enter was held -- a key it
/// never saw pressed, typed at the lock screen.
///
/// Against `2b6550e`, fails at "told a key is held".
#[test]
fn the_key_that_unlocks_is_not_handed_to_the_window() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();

    // Enter, which is evdev 28 and 36 to xkb, down at the lock
    // screen when the lock client unlocks.
    crate::input::key(&mut session.state, Keycode::new(36), KeyState::Pressed, 1);
    lock.unlock_and_destroy();
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.state.lock.is_none(),
        "the lock client unlocked and the session did not"
    );
    assert!(
        session.app.client.keyboard_on.is_none() || !session.app.client.enter_keys.contains(&28),
        "the window was told a key is held that it never saw pressed: \
         the Enter typed at the lock screen"
    );

    session.app.client.keys.clear();
    crate::input::key(&mut session.state, Keycode::new(36), KeyState::Released, 2);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.app.client.keyboard_on.is_some(),
        "the key came up and the keyboard did not go back to the window"
    );
    assert!(
        !session.app.client.enter_keys.contains(&28),
        "the window was told a key is held that it never saw pressed"
    );
    assert!(
        !session.app.client.keys.contains(&28),
        "the window was sent the release of a key it never saw pressed"
    );
    let (app, _) = session.type_key();
    assert!(app, "unlocked, and typing does not reach the application");
}

/// **The wheel over a hosted scene is not the scene's while the
/// session is locked**, as the pointer's motion and presses are not:
/// nothing of the session's may notice the pointer going past.
/// Tested with the Cyrillic group active (#132); the scene is stood
/// in for, beside these real clients (the #99 test).
#[test]
fn the_wheel_over_a_hosted_scene_is_not_the_scenes_while_locked() {
    use crate::qml::hosted::PointerKind;
    let mut session = Session::new();
    russian(&mut session.state);
    let bar = stand_in(
        &mut session.state,
        "bar",
        crate::scripted::Layer::Top,
        Rectangle::new((0, 0).into(), (1920, 30).into()),
        crate::scripted::Stand::solid(),
    );
    let wheels = |state: &Solium| {
        scene_events(state, bar)
            .into_iter()
            .filter(|event| matches!(event.kind, PointerKind::Wheel { .. }))
            .count()
    };
    move_pointer(&mut session.state, (100.0, 15.0), 1);
    let region = crate::monitor::union(&session.state.space).expect("a monitor");
    crate::synth::send_axis(&mut session.state, region, (0.0, 120.0), 2);
    assert_eq!(
        wheels(&session.state),
        1,
        "the premise: unlocked, the wheel over the scene is the scene's"
    );

    let _lock = session.lock();
    crate::synth::send_axis(&mut session.state, region, (0.0, 120.0), 3);
    assert_eq!(
        wheels(&session.state),
        1,
        "the scene heard the wheel behind the lock"
    );
}

/// **The lock lets go of a hosted scene's press and its hover**:
/// behind the lock nothing of the session's may notice the pointer,
/// so a press a scene held (Ruling 7) is not still held when the
/// session unlocks: the scene is told the buttons went up, as a
/// release of no button with none held, which cancels the press
/// inside it rather than clicking it
/// (`qml::hosted::tests::a_press_let_go_of_unseen_is_cancelled_not_clicked`),
/// and that the pointer left. Tested
/// with the Cyrillic group active (#132); the scene is stood in for,
/// beside these real clients (the #99 test).
#[test]
fn the_lock_lets_go_of_a_hosted_scenes_press_and_its_hover() {
    let mut session = Session::new();
    russian(&mut session.state);
    let bar = stand_in(
        &mut session.state,
        "bar",
        crate::scripted::Layer::Top,
        Rectangle::new((0, 0).into(), (1920, 30).into()),
        crate::scripted::Stand::solid(),
    );
    move_pointer(&mut session.state, (100.0, 15.0), 1);
    let region = crate::monitor::union(&session.state.space).expect("a monitor");
    crate::synth::send_button(
        &mut session.state,
        region,
        0x110,
        smithay::backend::input::ButtonState::Pressed,
        2,
    );
    assert!(
        session.state.scene_press.is_some(),
        "the premise: the scene holds the press"
    );

    let _lock = session.lock();
    let left = session
        .state
        .surfaces
        .get(bar)
        .and_then(crate::scripted::Surface::stand)
        .map(|stand| stand.left);
    let last = scene_events(&session.state, bar)
        .last()
        .map(|event| (event.kind, event.buttons));
    assert_eq!(
        (session.state.scene_press.is_none(), last, left),
        (
            true,
            Some((crate::qml::hosted::PointerKind::Release(0), 0)),
            Some(1)
        ),
        "(the hold let go, the last the scene was told, which ends its press as Qt \
         ends one let go of unseen, how many times it heard the pointer leave)"
    );
}

/// **No scene holds the keyboard while the session is locked**: a hold
/// ends at the lock, the scene is told to let go, none is taken
/// behind it, and one a scene asked for there is taken once the lock
/// is gone, as a grab is (Ruling 14). Tested with the Cyrillic group
/// active (#132); the scene is stood in for, beside these real
/// clients (the #99 test).
#[test]
fn no_scene_holds_the_keyboard_while_the_session_is_locked() {
    let mut session = Session::new();
    russian(&mut session.state);
    let shell = stand_in(
        &mut session.state,
        "shell",
        crate::scripted::Layer::Top,
        Rectangle::new((0, 0).into(), (1920, 1080).into()),
        crate::scripted::Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            ..crate::scripted::Stand::solid()
        },
    );
    let want = |session: &mut Session| {
        if let Some(stand) = session
            .state
            .surfaces
            .get_mut(shell)
            .and_then(crate::scripted::Surface::stand_mut)
        {
            stand.keyboard = Some(crate::qml::hosted::KeyboardReport::Wanted(Vec::new()));
        }
        session.state.settle_scenes();
    };
    want(&mut session);
    let before = session.state.hosted_keyboard.is_some();
    let lock = session.lock();
    let at_the_lock = session.state.hosted_keyboard.is_none();
    let told = session
        .state
        .surfaces
        .get(shell)
        .and_then(crate::scripted::Surface::stand)
        .map_or(0, |stand| stand.let_go);
    want(&mut session);
    let behind = (
        session.state.hosted_keyboard.is_some(),
        session.keyboard_on_lock(),
    );
    lock.unlock_and_destroy();
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    session.state.settle_scenes();
    assert_eq!(
        (
            before,
            at_the_lock,
            told,
            behind,
            session.state.lock.is_none(),
            session.state.hosted_keyboard.is_some()
        ),
        (true, true, 1, (false, true), true, true),
        "(held before the lock, let go at it, the scene told so, (held behind the \
         lock, the keyboard on the lock), unlocked, held after it)"
    );
}

/// **No hosted grab is held behind the lock, and one a scene took
/// there is held once the lock is gone** (Ruling 12): behind the lock
/// the pointer is the lock screen's, and a popup the shell opened
/// meanwhile has it after the unlock. Tested with the Cyrillic group
/// active (#132); the scene is stood in for, beside these real
/// clients (the #99 test).
#[test]
fn a_grab_a_scene_takes_behind_the_lock_is_held_once_it_is_gone() {
    let mut session = Session::new();
    russian(&mut session.state);
    let menu = stand_in(
        &mut session.state,
        "menu",
        crate::scripted::Layer::Top,
        Rectangle::new((0, 0).into(), (1920, 1080).into()),
        crate::scripted::Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            ..crate::scripted::Stand::solid()
        },
    );
    let lock = session.lock();
    if let Some(stand) = session
        .state
        .surfaces
        .get_mut(menu)
        .and_then(crate::scripted::Surface::stand_mut)
    {
        stand.grab = Some(crate::qml::hosted::GrabReport::Held("tray-menu".to_owned()));
    }
    session.state.settle_scenes();
    let behind = session.state.hosted_grab.is_some();
    lock.unlock_and_destroy();
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    session.state.settle_scenes();
    assert_eq!(
        (
            behind,
            session.state.lock.is_none(),
            session.state.hosted_grab.is_some()
        ),
        (false, true, true),
        "(a grab held behind the lock, the session unlocked, the grab held after it)"
    );
}

/// **A button let go of behind the lock is not held after it**: the
/// buttons held are counted whether or not the session is locked, so
/// a scene pressed after the unlock is told only what is held then;
/// and behind the lock the scene hears nothing of the release, the
/// motion and the right click there. Tested with the Cyrillic group
/// active (#132); the scene is stood in for, beside these real
/// clients (the #99 test).
#[test]
fn a_button_let_go_behind_the_lock_is_not_held_after_it() {
    use crate::qml::hosted::PointerKind;
    use smithay::backend::input::ButtonState;
    let mut session = Session::new();
    russian(&mut session.state);
    let bar = stand_in(
        &mut session.state,
        "bar",
        crate::scripted::Layer::Top,
        Rectangle::new((0, 0).into(), (1920, 30).into()),
        crate::scripted::Stand::solid(),
    );
    move_pointer(&mut session.state, (100.0, 15.0), 1);
    let region = crate::monitor::union(&session.state.space).expect("a monitor");
    crate::synth::send_button(&mut session.state, region, 0x110, ButtonState::Pressed, 2);

    let lock = session.lock();
    let at_the_lock = scene_events(&session.state, bar).len();
    crate::synth::send_button(&mut session.state, region, 0x110, ButtonState::Released, 3);
    move_pointer(&mut session.state, (200.0, 15.0), 4);
    crate::synth::send_button(&mut session.state, region, 0x111, ButtonState::Pressed, 5);
    crate::synth::send_button(&mut session.state, region, 0x111, ButtonState::Released, 6);
    let behind = scene_events(&session.state, bar).len() - at_the_lock;

    lock.unlock_and_destroy();
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.state.lock.is_none(),
        "the premise: the lock client unlocked and the session did too"
    );
    crate::synth::send_button(&mut session.state, region, 0x111, ButtonState::Pressed, 7);
    let last = scene_events(&session.state, bar)
        .last()
        .map(|event| (event.kind, event.buttons));
    assert_eq!(
        (behind, last),
        (0, Some((PointerKind::Press(0x2), 0x2))),
        "(what the scene heard behind the lock, what it was told of the right press \
         after it, and the buttons held then)"
    );
}

/// **Nothing captures the screen while the session is locked.**
/// What is on it then is the lock screen, and a recording of that
/// is the password's length and the rhythm it was typed at. Every
/// client but the lock client is behind the lock and any of them
/// can ask. Both ways in are driven: a capture asked for while
/// locked, and one asked for before the lock and copied after. A
/// capture already *queued* when the session locks is refused in
/// `screencopy::settle`, which needs a renderer and is not reached
/// from here.
///
/// Against `2b6550e`, fails at "was offered a buffer".
#[test]
fn nothing_captures_the_screen_while_the_session_is_locked() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let manager = session
        .app
        .client
        .screencopy
        .clone()
        .expect("zwlr_screencopy_manager_v1 bound");
    let output = session.app.client.output.clone().expect("wl_output bound");
    let before = manager.capture_output(0, &output, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(
        session.app.client.captures_offered, 1,
        "unlocked, a capture was not offered a buffer, so refusing one \
         proves nothing"
    );
    let lock = session.lock();

    let _during = manager.capture_output(0, &output, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(
        session.app.client.captures_offered, 1,
        "a capture asked for while locked was offered a buffer: an \
         application can record the lock screen"
    );
    assert_eq!(
        session.app.client.captures_failed, 1,
        "and it was never told it failed"
    );

    let shm = session.app.client.shm.clone().expect("wl_shm bound");
    let fd = anon_file(64 * 64 * 4);
    let pool = shm.create_pool(fd.as_fd(), 64 * 64 * 4, &session.app.qh, ());
    let buffer = pool.create_buffer(
        0,
        64,
        64,
        64 * 4,
        wl_shm::Format::Xrgb8888,
        &session.app.qh,
        (),
    );
    before.copy(&buffer);
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(
        session.app.client.captures_failed, 2,
        "a capture asked for before the lock and copied after it was \
         not refused"
    );

    session.assert_unlocks(lock);
}

/// **A menu that closes lets go of its grab.** `popup_grab` was set
/// when a menu grabbed and cleared only by `release_grabs` at the
/// next lock, so a menu closed an hour before was still recorded
/// as the chain holding the seat when the session next locked.
/// Harmless as things stand -- see `popup_destroyed` -- and tested
/// because the field's one reader is the lock.
///
/// Against `2b6550e`, fails at "still recorded".
#[test]
fn a_menu_that_closes_lets_go_of_its_grab() {
    let mut session = Session::new();
    let (_window, _toplevel, _surface, parent) =
        session.app.open(&mut session.display, &mut session.state);
    let menu = session
        .app
        .menu(&mut session.display, &mut session.state, &parent);
    assert!(
        session.state.popup_grab.is_some(),
        "the menu's grab was never recorded, so this proves nothing"
    );

    menu.destroy();
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.state.popup_grab.is_none(),
        "a menu closed and its grab is still recorded as the one \
         holding the seat"
    );
}

/// **A close in flight across a lock and its unlock never hands a
/// window the keyboard.** #127 gives a closing window back from
/// three places -- a refusal's deadline, a dialog answering the
/// close, and `settle_closing`'s retry -- and every one of them
/// ends in `settle_focus`, the call that once handed a locked
/// session's keyboard to the top application.
///
/// Locked, each step is measured the way every route in this module
/// is (`assert_sealed`): the close's deadline, a dialog's answer, a
/// refusal's deadline. Then the same two closes are in flight when
/// the lock client unlocks with Enter still down, which is
/// `refocus_on_release`'s tenth of a second. A dialog and a
/// refusal both land in it, and neither may give a window the
/// keyboard until the key is up, or that window is told in
/// `wl_keyboard.enter` that Enter is held. The retry goes through
/// the same `give_back` and needs a busy transform slot to reach;
/// it is not driven here.
///
/// Against this merge without `settle_focus`'s
/// `refocus_on_release` return, fails at "a dialog's answer was
/// handed the keyboard while the unlock key was held".
#[test]
fn a_close_in_flight_across_a_lock_never_takes_the_keyboard() {
    let mut session = Session::new();
    let (refused, _refused_top, ..) = session.app.open(&mut session.display, &mut session.state);
    let (answered, answered_top, ..) = session.app.open(&mut session.display, &mut session.state);
    let refused = session
        .state
        .panes
        .id_of(&refused)
        .expect("an open window has a pane");
    let answered = session
        .state
        .panes
        .id_of(&answered)
        .expect("an open window has a pane");
    let leaving = |state: &Solium, pane: crate::pane::PaneId| {
        state.panes.get(pane).is_some_and(Pane::leaving)
    };

    // super+q on both, and the lock screen straight after.
    session.state.close_pane(refused);
    session.state.close_pane(answered);
    let lock = session.lock();
    let selections = session.app.client.selections;

    // The close's deadline: the requests go out.
    session
        .state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = session.state.clock.now();
    session.state.settle_closing(asked);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        [refused, answered].into_iter().all(|pane| {
            session
                .state
                .panes
                .get(pane)
                .is_some_and(|pane| pane.asked_at().is_some())
        }),
        "both closes were asked, or nothing below is a close in flight"
    );
    session.assert_sealed("a close's deadline passed while locked", selections);

    // One of them answered with a dialog, which gives it back.
    let (_dialog, dialog_top, ..) = session.app.open(&mut session.display, &mut session.state);
    dialog_top.set_parent(Some(&answered_top));
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        !leaving(&session.state, answered),
        "the dialog did not give its parent back, so this proves nothing"
    );
    session.assert_sealed("a dialog answered a close while locked", selections);

    // The other said nothing, and its deadline gives it back.
    session.state.clock.advance(Duration::from_millis(1100));
    let now = session.state.clock.now();
    session.state.settle_refused(now);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        !leaving(&session.state, refused),
        "the refusal's deadline did not give the window back, so this \
         proves nothing"
    );
    session.assert_sealed("a refused close was given back while locked", selections);

    // Both closed again and asked, a second dialog open to answer
    // one of them -- opened while locked, so it takes no keyboard --
    // and Enter (evdev 28, 36 to xkb) down at the lock screen when
    // it unlocks.
    let (_second, second_top, ..) = session.app.open(&mut session.display, &mut session.state);
    session.state.close_pane(refused);
    session.state.close_pane(answered);
    session
        .state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = session.state.clock.now();
    session.state.settle_closing(asked);
    crate::input::key(&mut session.state, Keycode::new(36), KeyState::Pressed, 3);
    lock.unlock_and_destroy();
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.state.lock.is_none() && session.state.refocus_on_release,
        "the premise: unlocked, with the keyboard waiting for the key \
         that unlocked to come up"
    );
    assert!(
        session.app.client.keyboard_on.is_none(),
        "the premise: nothing has the keyboard while the key is down"
    );

    second_top.set_parent(Some(&answered_top));
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        !leaving(&session.state, answered),
        "the second dialog did not give its parent back, so this \
         proves nothing"
    );
    assert!(
        session.app.client.keyboard_on.is_none(),
        "a dialog's answer was handed the keyboard while the unlock key \
         was held, and its window told Enter is down: {:?}",
        session.app.client.enter_keys
    );

    session.state.clock.advance(Duration::from_millis(1100));
    let now = session.state.clock.now();
    session.state.settle_refused(now);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        !leaving(&session.state, refused),
        "the refusal's deadline did not give the window back, so this \
         proves nothing"
    );
    assert!(
        session.app.client.keyboard_on.is_none(),
        "a refusal's deadline was handed the keyboard while the unlock \
         key was held, and its window told Enter is down: {:?}",
        session.app.client.enter_keys
    );

    // The key comes up, and only now does a window take the keyboard.
    session.app.client.keys.clear();
    crate::input::key(&mut session.state, Keycode::new(36), KeyState::Released, 4);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.app.client.keyboard_on.is_some(),
        "the key came up and the keyboard did not go back to a window"
    );
    assert!(
        !session.app.client.enter_keys.contains(&28),
        "the window was told a key is held that it never saw pressed"
    );
    assert!(
        !session.app.client.keys.contains(&28),
        "the window was sent the release of a key it never saw pressed"
    );
    let (app, _) = session.type_key();
    assert!(app, "unlocked, and typing does not reach the application");
}

/// **A window closed with a menu open takes the menu's grab with
/// it.** #127 takes the keyboard off a closing window once its
/// animation lands, by giving the keyboard to nothing. With a menu
/// open that did nothing: the menu's keyboard grab ignores every
/// `set_focus` until its chain ends, and `focused_window` answers
/// for the menu with its window, so `settle_focus` saw a focused
/// window and left it. The keyboard stayed on the closed window's
/// menu for the whole grace period, and every key typed after the
/// close went to it.
/// `release_grabs_of` is the fix, and `release_grabs` -- which
/// does the same for everyone when the session locks -- the model.
///
/// Against this merge without `release_grabs_of` in
/// `hand_off_keyboard`, fails at "the menu's keyboard grab
/// outlived its window's close".
#[test]
fn a_close_with_a_menu_open_takes_the_keyboard_off_the_menu() {
    let mut session = Session::new();
    let (kept, ..) = session.app.open(&mut session.display, &mut session.state);
    let (closing, _toplevel, _surface, parent) =
        session.app.open(&mut session.display, &mut session.state);
    let _menu = session
        .app
        .menu(&mut session.display, &mut session.state, &parent);
    let keyboard = session
        .state
        .seat
        .get_keyboard()
        .expect("the seat has a keyboard");
    assert!(
        keyboard.is_grabbed() && session.state.is_focused(&closing),
        "the premise: a menu open on the focused window, holding a grab"
    );

    let pane = session
        .state
        .panes
        .id_of(&closing)
        .expect("an open window has a pane");
    session.state.close_pane(pane);
    session
        .state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = session.state.clock.now();
    session.state.settle_closing(asked);
    session.app.pump(&mut session.display, &mut session.state);

    assert!(
        !keyboard.is_grabbed(),
        "the menu's keyboard grab outlived its window's close"
    );
    assert!(
        session.app.client.popups_done > 0,
        "the closed window's menu was left open"
    );
    assert_eq!(
        session.state.focused_window().as_ref(),
        Some(&kept),
        "the keyboard did not move to the window that is still drawn"
    );
    let (app, _) = session.type_key();
    assert!(app, "the key reached nobody at all");
    assert_eq!(
        session
            .app
            .client
            .typed
            .last()
            .and_then(|(surface, _)| *surface),
        Some(surface_id(&kept)),
        "a key typed after the close did not reach the window that is \
         drawn"
    );
}

/// **The rule the X11 clipboard bridge asks: no X11 client may
/// read the selection while the session is locked.** There is no
/// focus to gate on the X11 side of the bridge, and every X11
/// client is behind the lock.
///
/// What this does *not* cover, said plainly because its first
/// version claimed it did: the bridge itself. It asks
/// `Solium::x11_may_read_selection` under a real lock taken by a
/// real lock client, and that is all. The one line that connects
/// the rule to X11 -- `XwmHandler::allow_selection_access` in
/// `xwayland.rs`, which returns it -- is not called here, because
/// it cannot be: it takes an `XwmId`, which smithay constructs only
/// inside a running X11 window manager, and there is no X server in
/// a unit test. Replacing that line with `true` would leave this
/// green. There is no X11 client here either.
#[test]
fn the_x11_selection_rule_refuses_while_the_session_is_locked() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    assert!(
        session.state.x11_may_read_selection(),
        "unlocked, the two halves of the session paste into each other"
    );
    let lock = session.lock();
    assert!(
        !session.state.x11_may_read_selection(),
        "locked, an X11 client could read whatever the lock screen copied"
    );
    session.assert_unlocks(lock);
    assert!(
        session.state.x11_may_read_selection(),
        "and unlocked again, it can"
    );
}

/// **#126 with #130: a window that closes itself behind the lock
/// hands the keyboard to nobody but the lock.**
///
/// The window goes on fading as it would unlocked -- it is drawn
/// by nothing while the screen is locked, `render::elements`
/// returns before any window -- and what it leaves has no surface
/// to be given the keyboard. The window going is a change to the
/// window list, which is when `settle_focus` runs, and while locked
/// that is the lock's own settle and nothing else.
#[test]
fn a_window_closing_itself_behind_the_lock_leaves_the_keyboard_on_it() {
    let mut session = Session::new();
    let _below = session.app.open(&mut session.display, &mut session.state);
    let (window, going, _, _) = session.app.open(&mut session.display, &mut session.state);
    let pane = session.state.panes.id_of(&window).expect("a pane");
    let selections = session.app.client.selections;
    let _lock = session.lock();

    going.destroy();
    session.app.pump(&mut session.display, &mut session.state);
    session.state.space.refresh();
    session.state.sync_panes();
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.state.panes.get(pane).is_some_and(Pane::ghost),
        "the premise: the window is fading out"
    );
    session.assert_sealed("a window that closed itself", selections);
}

/// **`locked` is not sent in the dispatch that asked for the lock**,
/// and everything else still is: `lock` set, the next frame built
/// under it, a frame asked for, the keyboard taken off the
/// application. Only the promise waits, because nothing has reached
/// a screen yet -- and `locked` used to go out right here, with the
/// monitor still scanning out the desktop.
///
/// Against this branch with `lock` confirming at once, as it did
/// before, fails at "before any monitor had shown the lock".
#[test]
fn locked_is_not_sent_in_the_dispatch_that_asked_for_it() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    assert!(
        session.app.client.keyboard_on.is_some(),
        "the application has the keyboard before the lock, or taking \
         it away proves nothing"
    );
    session.state.redraw = false;

    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    session.app.pump(&mut session.display, &mut session.state);

    assert!(
        session.state.lock.is_some(),
        "the lock client asked and the session did not lock"
    );
    assert!(
        session.state.lock_frame().is_some(),
        "the next frame is not built under the lock: the blanking waits \
         as well"
    );
    assert!(
        session.state.redraw,
        "no frame was asked for, so nothing replaces the desktop"
    );
    assert!(
        session.app.client.keyboard_on.is_none(),
        "the application kept the keyboard while `locked` waits"
    );
    assert!(
        !session
            .locker
            .client
            .locked
            .contains(&wayland_client::Proxy::id(&lock)),
        "the lock client was told `locked` in the dispatch that asked \
         for it, before any monitor had shown the lock"
    );
}

/// **`locked` waits for every monitor to show the lock.** One
/// monitor showing it, however many frames it shows, is one monitor:
/// the other is still scanning out whatever it had.
///
/// Against the old confirmation, fails at "had shown nothing".
#[test]
fn locked_waits_for_every_monitor_to_show_the_lock() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;

    for _ in 0..3 {
        session.show(&left);
    }
    assert!(
        !session.told_locked(&lock),
        "the lock client was told `locked` while the right-hand monitor \
         had shown nothing of the lock"
    );
    session.show(&right);
    assert!(
        session.told_locked(&lock),
        "every monitor showed the lock and the lock client was not told \
         `locked`"
    );
}

/// **A flip already in flight when the session locks does not
/// count**, and nor does a frame built under the lock this one took
/// over from. `tty.rs` skips a screen whose flip is pending, so the
/// first flip to complete after a lock can be a frame queued before
/// it: the desktop. Each is handed back here the way a vblank hands
/// it back, with the lock it was built under, before the lock's own
/// frame.
///
/// What this plays is `Solium`'s half. The tty half -- that each
/// frame's own user data comes back out of `frame_submitted` -- needs
/// a GPU and is not reached here.
///
/// Against the old confirmation, fails at "on the desktop's flip".
/// Against a fix that counted any frame presented after the lock,
/// fails at the same line.
#[test]
fn a_flip_already_in_flight_at_the_lock_does_not_count() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let monitor = session.output(0);
    // Built, and queued, before the lock.
    let in_flight = session.state.lock_frame();
    assert!(
        in_flight.is_none(),
        "the premise: a frame built before the lock carries no lock"
    );

    let (first, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    session.state.frame_presented(&monitor, in_flight);
    assert!(
        !session.told_locked(&first),
        "the lock client was told `locked` on the desktop's flip: the \
         frame that reached the screen was built before the lock"
    );
    session.show(&monitor);
    assert!(
        session.told_locked(&first),
        "the lock's own frame reached the screen and the lock client \
         was not told `locked`"
    );

    // The lock client crashes with a frame of its lock in flight, and
    // a new one takes over before that frame flips.
    let in_flight = session.state.lock_frame();
    let recovery = Side::connect(&mut session.display, &mut session.state);
    let crashed = std::mem::replace(&mut session.locker, recovery);
    session.lock_surfaces.clear();
    drop(crashed);
    session.app.pump(&mut session.display, &mut session.state);
    let (second, surfaces) =
        session
            .locker
            .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    assert!(
        session.state.lock_frame().is_some() && session.state.lock_frame() != in_flight,
        "the premise: the new lock is not the one that frame was built under"
    );
    session.state.frame_presented(&monitor, in_flight);
    assert!(
        !session.told_locked(&second),
        "the new lock client was told `locked` on a frame built under \
         the lock it took over from"
    );
    session.show(&monitor);
    assert!(
        session.told_locked(&second),
        "the new lock's own frame reached the screen and its lock client \
         was not told `locked`"
    );
}

/// **A monitor switched off does not hold `locked` back.** Switched
/// off with `enabled = false`, that is, which `tty.rs` drops just as
/// it drops one unplugged. A lock asked for after that waits for the
/// monitor that is still on, and for nothing else. A monitor powered
/// off is not dropped -- it stays, dark -- and is
/// `power::a_dark_monitor_does_not_hold_locked_back`'s.
///
/// Against the old confirmation, fails at "the one still on had
/// shown".
#[test]
fn a_switched_off_monitor_does_not_hold_locked_back() {
    let mut session = Session::with_monitors(2);
    let on = session.output(0);
    session.unplug(1);
    session
        .locker
        .pump(&mut session.display, &mut session.state);

    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    assert!(
        !session.told_locked(&lock),
        "the lock client was told `locked` before the one still on had \
         shown the lock"
    );
    session.show(&on);
    assert!(
        session.told_locked(&lock),
        "the monitor still on showed the lock, and `locked` waits for \
         the one switched off"
    );
}

/// **A monitor unplugged while `locked` waits does not hold it back
/// for ever.** The left-hand one has shown the lock, the right-hand
/// one never will because it has gone, and `locked` goes out as it
/// goes, with no frame after.
///
/// Against the old confirmation, fails at "had shown nothing".
#[test]
fn a_monitor_unplugged_while_locking_does_not_hold_locked_back() {
    let mut session = Session::with_monitors(2);
    let left = session.output(0);
    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    session.show(&left);
    assert!(
        !session.told_locked(&lock),
        "the lock client was told `locked` while the right-hand monitor \
         had shown nothing of the lock"
    );

    session.unplug(1);
    assert!(
        session.told_locked(&lock),
        "the monitor `locked` was waiting for was unplugged, and the lock \
         client is waiting still"
    );
}

/// **With no monitor, `locked` goes out at once**: there is no
/// screen for anything to be on. Both ways there can be none --
/// none when the lock is asked for, and the only one unplugged
/// before it showed the lock.
///
/// The first half is what the old confirmation did as well, and
/// passes against it. The second fails against it at "the
/// premise".
#[test]
fn with_no_monitor_locked_goes_out_at_once() {
    let mut session = Session::with_monitors(0);
    let (lock, _) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    assert!(
        session
            .locker
            .client
            .locked
            .contains(&wayland_client::Proxy::id(&lock)),
        "with no monitor to wait for, the lock client was not told \
         `locked` in the dispatch that asked for it"
    );

    let mut session = Session::new();
    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    assert!(
        !session.told_locked(&lock),
        "the premise: with a monitor, `locked` waits for it"
    );
    session.unplug(0);
    assert!(
        session.told_locked(&lock),
        "the only monitor was unplugged before it showed the lock, and \
         the lock client is waiting still"
    );
}

/// **A monitor plugged in while `locked` waits must show the lock
/// too.** It is a screen like the others, and `locked` is a promise
/// about every screen there is.
///
/// Against the old confirmation, fails at "had shown nothing".
#[test]
fn a_monitor_plugged_in_while_locking_must_show_the_lock_too() {
    let mut session = Session::new();
    let first = session.output(0);
    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;

    let second = session.plug();
    session.show(&first);
    assert!(
        !session.told_locked(&lock),
        "the lock client was told `locked` while the monitor plugged in \
         had shown nothing of the lock"
    );
    session.show(&second);
    assert!(
        session.told_locked(&lock),
        "both monitors showed the lock and the lock client was not told \
         `locked`"
    );
}

/// **A lock client that goes before it is told `locked` leaves the
/// session locked**, and a new one can take over. Both ways it can
/// go: giving up with `destroy`, which the protocol allows before
/// `locked`, and crashing. Neither unlocks anything, the `locked` it
/// was owed is kept and never spent, and the lock client that takes
/// over is told `locked` for its own lock once that is on the monitor.
///
/// What this cannot see is what the owed `locked` sends when it is
/// dropped at the takeover: nothing, but a client discards events on
/// an object it has destroyed and a crashed one reads nothing, so no
/// client here could tell. That half rests on `destroyed` in
/// `lock.rs`.
///
/// Against the old confirmation, fails at "the premise". With
/// `confirm_lock` confirming a lock whose client has gone, fails at
/// "was spent".
#[test]
fn a_lock_client_that_goes_before_locked_leaves_the_session_locked() {
    for crash in [false, true] {
        let route = if crash { "crashed" } else { "gave up" };
        let mut session = Session::new();
        session.app.open(&mut session.display, &mut session.state);
        let selections = session.app.client.selections;
        let (lock, surfaces) =
            session
                .locker
                .ask_lock(&mut session.display, &mut session.state, true);
        assert!(
            !session.told_locked(&lock),
            "{route}: the premise: the lock client has not been told \
             `locked`"
        );

        // Its replacement is connected first either way, the way one
        // is started from another terminal. One that gave up stays
        // connected throughout.
        let recovery = Side::connect(&mut session.display, &mut session.state);
        let mut going = std::mem::replace(&mut session.locker, recovery);
        let _gave_up = if crash {
            drop(going);
            None
        } else {
            for surface in &surfaces {
                surface.destroy();
            }
            lock.destroy();
            let error = going.pump_or_error(&mut session.display, &mut session.state);
            assert!(
                error.is_none(),
                "{route}: `destroy` before `locked` is legal, and the lock \
                 client was disconnected for it: {error:?}"
            );
            Some(going)
        };
        session.app.pump(&mut session.display, &mut session.state);
        // Frames go on reaching the screen, and there is nobody to
        // tell.
        session.show_all();
        session.app.pump(&mut session.display, &mut session.state);
        assert!(
            session
                .state
                .lock
                .as_ref()
                .is_some_and(crate::lock::Lock::pending),
            "{route}: the `locked` owed to a lock client that has gone was \
             spent, on nobody"
        );

        assert!(
            session.state.lock.is_some(),
            "{route}: the lock client went before `locked` and the \
             session unlocked"
        );
        assert!(
            session.app.client.keyboard_on.is_none(),
            "{route}: and the application was given the keyboard"
        );
        let (app, _) = session.type_key();
        assert!(!app, "{route}: and a key reached the application");

        let lock = session.lock();
        session.assert_sealed(&format!("{route}, then a new lock took over"), selections);
        session.assert_unlocks(lock);
    }
}

/// **An unlock before `locked` unlocks nothing**, from the lock's
/// own holder as from anyone. It is `invalid_unlock` -- the
/// protocol's error for unlocking a lock never told `locked` -- and
/// smithay posts it and then unlocks anyway, for the holder as for
/// the intruder in
/// `an_unlock_from_a_lock_that_was_never_granted_unlocks_nothing`.
/// While `locked` went out in the dispatch that granted the lock the
/// holder could not get here; now it can, and a lock client thrown
/// off for a protocol error leaves the session locked like one that
/// crashed.
///
/// Against the old confirmation, fails at "the premise". With the
/// confirmation fixed and the holder's unlock passed to smithay
/// untouched, fails at "unlocked the session".
#[test]
fn an_unlock_before_locked_unlocks_nothing() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let selections = session.app.client.selections;
    let (lock, _surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    assert!(
        !session
            .locker
            .client
            .locked
            .contains(&wayland_client::Proxy::id(&lock)),
        "the premise: the lock client has not been told `locked`"
    );

    lock.unlock_and_destroy();
    let error = session
        .locker
        .pump_or_error(&mut session.display, &mut session.state);
    assert!(
        session.state.lock.is_some(),
        "the lock client unlocked the session before it was told `locked`"
    );
    let error = error.expect(
        "the lock client was not disconnected: an unlock before `locked` \
         is `invalid_unlock`",
    );
    assert_eq!(
        error.code,
        ext_session_lock_v1::Error::InvalidUnlock as u32,
        "disconnected, but not for `invalid_unlock`: {error:?}"
    );

    // Thrown off, it has gone the way a crashed one goes: a new lock
    // client takes over.
    session.locker = Side::connect(&mut session.display, &mut session.state);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.app.client.keyboard_on.is_none(),
        "the lock client was thrown off and the application was given \
         the keyboard"
    );
    let lock = session.lock();
    session.assert_sealed("an unlock before `locked`, then a new lock", selections);
    session.assert_unlocks(lock);
}

mod native;

/// **#54: turning a monitor off** -- over `wlr-output-power-management`,
/// from a script, and on its own when nobody is there -- and what the
/// lock does with a monitor that is dark.
///
/// Here, beside the lock's tests, for their fixture: the lock halves
/// need the lock client, and the rest need no more than the
/// application's client with a window.
mod image_copy;

mod power;

mod frames;

/// **Escape is the focused window's, except while the overview is
/// up.** #174.
///
/// `overview.lua` bound `escape` for good, and the key filter takes
/// every bound press before any client sees it, so Escape reached no
/// application at all. Each test drives the shipped `overview.lua`, a
/// real application client with the keyboard, and keys pressed
/// through the filter the backends feed, on `us,ru` with each half
/// active (#132). Each fails against `19a42c0`; the doc of each says
/// where.
mod escape {
    use super::*;

    /// Escape, Super and space, as xkb keycodes: evdev 1, 125 and
    /// 57, plus eight.
    const ESCAPE: u32 = 9;
    const SUPER: u32 = 133;
    const SPACE: u32 = 65;

    /// Both halves of `us,ru`, one-based as `sol.keyboard` counts.
    const GROUPS: [(&str, usize); 2] = [("us", 1), ("ru", 2)];

    /// One window with the keyboard and the shipped `overview.lua`,
    /// on `us,ru` with its `active`th layout locked. The entry point
    /// comes back so a test can read it again, which is a reload.
    fn overview(tag: &str, active: usize) -> (Session, std::path::PathBuf) {
        let mut session = Session::new();
        session.app.open(&mut session.display, &mut session.state);
        let directory = std::env::temp_dir().join(format!(
            "solium-escape-{tag}-{active}-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n\
                 sol.keyboard{{ layout = \"us,ru\", active = {active} }}\n\
                 require(\"overview\")\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
        session.state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the shipped overview"),
        ));
        session.app.pump(&mut session.display, &mut session.state);
        let keyboard = crate::keymap::describe(&mut session.state);
        assert_eq!(
            (keyboard.layouts.len(), keyboard.active),
            (2, active),
            "the premise: `us,ru` with layout {active} locked; it is {keyboard:?}"
        );
        assert!(
            session.app.client.keyboard_on.is_some(),
            "the premise: the window has the keyboard"
        );
        (session, entry)
    }

    /// Press `keys` in order and let them go in reverse, through the
    /// filter, and answer how many Escape presses the window was
    /// sent.
    fn chord(session: &mut Session, keys: &[u32]) -> usize {
        session.app.client.key_events.clear();
        for &code in keys {
            crate::input::key(&mut session.state, Keycode::new(code), KeyState::Pressed, 0);
        }
        for &code in keys.iter().rev() {
            crate::input::key(
                &mut session.state,
                Keycode::new(code),
                KeyState::Released,
                0,
            );
        }
        session.app.pump(&mut session.display, &mut session.state);
        session
            .app
            .client
            .key_events
            .iter()
            .filter(|&&(key, down)| key == 1 && down)
            .count()
    }

    /// Whether the overview is up: it holds the input, and says so.
    fn up(session: &Session) -> bool {
        session.state.script_grab && session.state.status == "overview"
    }

    /// What `Solium::reload` does with the scripts, read from `entry`
    /// rather than from the user's configuration: the keep carried
    /// over, the scripts started, then `restore`, `monitors` and
    /// `layout`.
    fn reload(session: &mut Session, entry: &std::path::Path) {
        let state = &mut session.state;
        let mut scripts = state.scripts.take().expect("the scripts loaded");
        scripts
            .reload(entry)
            .expect("reloading the shipped overview");
        state.start_scripts(Some(scripts));
        let snapshot = state.snapshot();
        let mut scripts = state.scripts.take().expect("the scripts reloaded");
        let restored = scripts.restored(snapshot);
        state.scripts = Some(scripts);
        state.apply(restored);
        state.trigger_monitors_changed();
        state.trigger_relayout();
    }

    /// **With the overview closed, Escape reaches the focused
    /// window.** Against `19a42c0`, fails at "kept from the window".
    #[test]
    fn escape_reaches_the_focused_window_while_the_overview_is_closed() {
        for (name, active) in GROUPS {
            let (mut session, _) = overview("closed", active);
            assert_eq!(
                chord(&mut session, &[ESCAPE]),
                1,
                "{name}: Escape was kept from the window with the overview closed"
            );
        }
    }

    /// **With the overview up, Escape leaves it and the window is
    /// sent no Escape; once it has left, Escape is the window's
    /// again.** Against `19a42c0`, fails at "still taken".
    #[test]
    fn escape_leaves_the_overview_and_only_then_reaches_the_window() {
        for (name, active) in GROUPS {
            let (mut session, _) = overview("open", active);
            chord(&mut session, &[SUPER, SPACE]);
            assert!(up(&session), "{name}: the premise: super+space entered");
            assert_eq!(
                chord(&mut session, &[ESCAPE]),
                0,
                "{name}: the window was sent the Escape that left the overview"
            );
            assert!(!up(&session), "{name}: Escape did not leave the overview");
            assert_eq!(
                chord(&mut session, &[ESCAPE]),
                1,
                "{name}: the overview has gone and Escape is still taken from the window"
            );
        }
    }

    /// **A reload with the overview up leaves Escape able to leave
    /// it.** The grab and the thumbnails outlive `super+shift+r`, and
    /// the Lua state does not. Against `19a42c0`, fails at "after a
    /// reload": the new `overview.lua` believed it was closed.
    #[test]
    fn escape_leaves_the_overview_after_a_reload() {
        for (name, active) in GROUPS {
            let (mut session, entry) = overview("reload", active);
            chord(&mut session, &[SUPER, SPACE]);
            assert!(up(&session), "{name}: the premise: super+space entered");
            reload(&mut session, &entry);
            assert!(
                up(&session),
                "{name}: the premise: the overview is still up after the reload"
            );
            assert_eq!(
                chord(&mut session, &[ESCAPE]),
                0,
                "{name}: the window was sent the Escape that left the overview"
            );
            assert!(
                !up(&session),
                "{name}: Escape did not leave the overview after a reload"
            );
            assert_eq!(
                chord(&mut session, &[ESCAPE]),
                1,
                "{name}: the overview has gone and Escape is still taken from the window"
            );
        }
    }
}

/// **No warp moves the pointer behind the lock** (#220): neither
/// `sol.warp` nor the warp of a `sol.focus` the lock refused takes
/// the pointer anywhere while the session is locked, so nothing of the
/// session's moves what the lock screen is being used with.
#[test]
fn no_warp_moves_the_pointer_behind_the_lock() {
    use crate::script::{FocusSource, Warp, WarpTo};
    let mut session = Session::new();
    let (window, _toplevel, _surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let id = session
        .state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane")
        .get();
    session.lock();
    let at = |session: &Session| {
        session
            .state
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default()
    };
    let before = at(&session);
    session.state.apply(Outcome {
        commands: vec![Command::Warp(Warp {
            to: WarpTo::Point {
                x: 1500.0,
                y: 800.0,
            },
            motion: None,
            interrupt: crate::script::Interrupt::Pointer,
        })],
        ..Outcome::default()
    });
    let warped = at(&session);
    session.state.apply(Outcome {
        commands: vec![Command::Focus {
            id,
            source: FocusSource::Script,
            warp: Some(Warp {
                to: WarpTo::Window { id, at: (0.5, 0.5) },
                motion: None,
                interrupt: crate::script::Interrupt::Pointer,
            }),
        }],
        ..Outcome::default()
    });
    assert_eq!(
        (warped, at(&session), session.state.flight.is_none()),
        (before, before, true),
        "(after `sol.warp`, after `sol.focus` with a warp, nothing flying): the pointer \
         moved behind the lock"
    );
}

/// **Nothing `sol.press` asks reaches a window behind the lock**: a
/// button and a key for the application's window, asked while the
/// session is locked, reach its client as neither, and the keyboard
/// stays on the lock surface.
#[test]
fn nothing_sol_press_asks_reaches_a_window_behind_the_lock() {
    let mut session = Session::new();
    let (window, _toplevel, _surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let id = session
        .state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane")
        .get();
    let _pointer = session.app.client.seat_pointer(&session.app.qh);
    session.app.pump(&mut session.display, &mut session.state);
    session.lock();
    session.app.client.buttons.clear();
    session.app.client.keys.clear();
    let press = |press| Command::Press {
        id,
        press,
        source: crate::script::FocusSource::Script,
    };
    session.state.apply(Outcome {
        commands: vec![
            press(crate::script::Press::Button(0x113)),
            press(crate::script::Press::Keys(vec![
                smithay::input::keyboard::Keysym::Escape,
            ])),
        ],
        ..Outcome::default()
    });
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(
        (
            session.app.client.buttons.clone(),
            session.app.client.keys.clone(),
            session.keyboard_on_lock(),
        ),
        (Vec::new(), Vec::new(), true),
        "(the buttons the application heard, its keys, the keyboard on the lock)"
    );
}

mod shortcuts_inhibit;

mod held;
