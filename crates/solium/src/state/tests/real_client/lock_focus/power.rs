use super::*;
use crate::lock::LockFrame;
use crate::power::{Lit, Step};

/// The tty's half, played the way `tty.rs` plays it: what each
/// monitor's display has been through, moved on by the same
/// `power::after` `tty.rs` calls, and the frame in flight on it
/// with the lock it was built under. Every call here goes
/// through, so the one the fixture cannot play is a failed one:
/// `every_way_a_monitor_goes_off_and_on_is_one_step_at_a_time`
/// has that.
#[derive(Default)]
struct Tty {
    lit: Vec<(Output, Lit)>,
    in_flight: Vec<(Output, Option<LockFrame>)>,
}

impl Tty {
    fn lit(&self, output: &Output) -> Lit {
        self.lit
            .iter()
            .find(|(each, _)| each == output)
            .map_or(Lit::On, |(_, lit)| *lit)
    }

    fn set(&mut self, output: &Output, lit: Lit) {
        self.lit.retain(|(each, _)| each != output);
        self.lit.push((output.clone(), lit));
    }

    /// Build and queue this monitor's next frame as `render` does:
    /// its picture, a frame of black, the display switched off, or
    /// nothing.
    fn queue(&mut self, state: &mut Solium, output: &Output) -> Step {
        assert!(
            !self.in_flight.iter().any(|(each, _)| each == output),
            "the fixture queued over a pending flip, which `render` never does"
        );
        let step = state.power_step(output, self.lit(output));
        match step {
            Step::Draw | Step::Wake | Step::Blank => {
                self.in_flight.push((output.clone(), state.lock_frame()));
            }
            Step::Darken => state.output_dark(output),
            Step::Rest => {}
        }
        self.set(output, crate::power::after(step, true));
        step
    }

    /// The flip of whatever is in flight on this monitor lands.
    fn flip(&mut self, state: &mut Solium, output: &Output) {
        if let Some(index) = self.in_flight.iter().position(|(each, _)| each == output) {
            let (_, frame) = self.in_flight.remove(index);
            state.frame_presented(output, frame);
        }
    }

    /// One frame, queued and on the screen.
    fn frame(&mut self, state: &mut Solium, output: &Output) -> Step {
        let step = self.queue(state, output);
        self.flip(state, output);
        step
    }

    /// Frames until this monitor is doing what it will go on
    /// doing, and what each one was.
    fn settle(&mut self, state: &mut Solium, output: &Output) -> Vec<Step> {
        let mut steps = Vec::new();
        for _ in 0..6 {
            let step = self.frame(state, output);
            steps.push(step);
            if matches!(step, Step::Draw | Step::Rest) {
                break;
            }
        }
        steps
    }
}

impl Session {
    /// A power control for the `index`th monitor, asked for by the
    /// application's client the way `wlopm` asks for one.
    pub(super) fn control(&mut self, index: usize) -> zwlr_output_power_v1::ZwlrOutputPowerV1 {
        let manager = self
            .app
            .client
            .output_power
            .clone()
            .expect("zwlr_output_power_manager_v1 is not advertised");
        let output = self
            .app
            .client
            .outputs
            .get(index)
            .cloned()
            .expect("a wl_output for that monitor");
        let control = manager.get_output_power(&output, &self.app.qh, ());
        self.app.pump(&mut self.display, &mut self.state);
        control
    }

    /// Every mode `control` has been told, oldest first: true is on.
    fn modes(&mut self, control: &zwlr_output_power_v1::ZwlrOutputPowerV1) -> Vec<bool> {
        self.app.pump(&mut self.display, &mut self.state);
        let id = wayland_client::Proxy::id(control);
        self.app
            .client
            .power_modes
            .iter()
            .filter(|(each, _)| *each == id)
            .map(|(_, on)| *on)
            .collect()
    }

    /// `wlopm --on` or `--off`.
    pub(super) fn power(&mut self, control: &zwlr_output_power_v1::ZwlrOutputPowerV1, on: bool) {
        control.set_mode(if on {
            zwlr_output_power_v1::Mode::On
        } else {
            zwlr_output_power_v1::Mode::Off
        });
        self.app.pump(&mut self.display, &mut self.state);
    }

    /// `idle.lock_after = 0`, for a test of the blank that would
    /// otherwise be locked half way through, and its keys given to
    /// the lock.
    fn without_the_idle_lock(&mut self) {
        let settings = self.state.idle.settings();
        self.state.idle.configure(crate::idle::Settings {
            lock_after: Duration::ZERO,
            ..settings
        });
    }

    /// Time passes with nobody touching anything, and a frame runs.
    fn wait(&mut self, by: Duration) {
        self.state.clock.advance(by);
        crate::idle::settle(&mut self.state);
        crate::logind::settle(&mut self.state);
        self.app.pump(&mut self.display, &mut self.state);
    }

    /// Whether every one of these monitors is off.
    fn all_off(&self, outputs: &[&Output]) -> bool {
        outputs.iter().all(|output| self.state.power.is_off(output))
    }

    /// Whether none of them is.
    fn none_off(&self, outputs: &[&Output]) -> bool {
        outputs
            .iter()
            .all(|output| !self.state.power.is_off(output))
    }
}

/// **The protocol is there, says the mode on bind, and turns a
/// monitor off and on** -- telling every control of that monitor
/// each time, and nobody when nothing changed. A monitor that goes
/// away is `failed` to its controls, and so is a control asked for
/// on one that has gone.
///
/// The old code advertises no such global, so fails at "not
/// advertised". With `settle_monitors` not forgetting a monitor
/// that went, fails at "was unplugged".
#[test]
fn the_power_protocol_is_advertised_answers_mode_on_bind_and_turns_a_monitor_off_and_on() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let control = session.control(0);
    assert_eq!(
        session.modes(&control),
        [true],
        "a control is told its monitor's mode the moment it is made"
    );

    session.power(&control, false);
    assert_eq!(
        session.modes(&control),
        [true, false],
        "set_mode(off) was not answered with `mode off`"
    );
    assert!(
        session.state.power.is_off(&left),
        "told off, and the monitor is on"
    );
    assert!(
        !session.state.power.is_off(&right),
        "the other monitor went off with it"
    );

    // A second control for the same monitor, as a panel would
    // hold: not refused, and told what the monitor is now.
    let second = session.control(0);
    assert_eq!(session.modes(&second), [false], "made while it is off");
    session.power(&control, false);
    assert_eq!(
        session.modes(&control),
        [true, false],
        "asked for what it already was, and told it again"
    );

    session.power(&second, true);
    assert_eq!(
        session.modes(&control),
        [true, false, true],
        "turned on through one control, and the other was not told"
    );
    assert_eq!(session.modes(&second), [false, true]);
    assert!(!session.state.power.is_off(&left));

    // The right-hand monitor is unplugged with a control on it.
    let on_right = session.control(1);
    let right_output = session
        .app
        .client
        .outputs
        .get(1)
        .cloned()
        .expect("a wl_output for the right-hand monitor");
    session.unplug(1);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session
            .app
            .client
            .power_failed
            .contains(&wayland_client::Proxy::id(&on_right)),
        "a control on a monitor that was unplugged was not told `failed`"
    );
    let late = session
        .app
        .client
        .output_power
        .clone()
        .expect("the manager")
        .get_output_power(&right_output, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session
            .app
            .client
            .power_failed
            .contains(&wayland_client::Proxy::id(&late)),
        "a control for a monitor that has gone was not told `failed`"
    );
}

/// **A monitor that is off is not drawn, and nothing about the
/// layout changes.** It goes black and then dark and is left alone
/// for as long as it is off; its place, its work area, the window
/// on it and its `wl_output` all stay exactly where they were, and a
/// script still sees it, marked off. Back on, it is one frame drawn
/// in full, and then frames as ever.
///
/// With going off switching the display off before any black,
/// fails at "a frame of black"; with a wake not drawn in full, at
/// "one frame drawn in full"; with the snapshot not saying so, at
/// "does not say it is off".
#[test]
fn an_off_monitor_is_not_drawn_and_nothing_about_the_layout_changes() {
    let mut session = Session::with_monitors(2);
    let (window, ..) = session.app.open(&mut session.display, &mut session.state);
    let left = session.output(0);
    let mut tty = Tty::default();
    let layout = |state: &Solium| {
        (
            state.space.outputs().count(),
            state.space.output_geometry(&left),
            state.work_area_on(&left),
            state.real_geometry(&window),
        )
    };
    let before = layout(&session.state);
    assert_eq!(tty.frame(&mut session.state, &left), Step::Draw);

    let control = session.control(0);
    session.power(&control, false);
    assert_eq!(
        tty.settle(&mut session.state, &left),
        [Step::Blank, Step::Darken, Step::Rest],
        "going off is a frame of black, the display switched off, and then nothing"
    );
    for _ in 0..3 {
        assert_eq!(
            tty.frame(&mut session.state, &left),
            Step::Rest,
            "a monitor that is off was drawn"
        );
    }
    assert!(session.state.power.is_dark(&left));

    assert_eq!(
        layout(&session.state),
        before,
        "turning a monitor off moved a monitor, a work area or a window"
    );
    let snapshot = session.state.snapshot();
    let row = snapshot
        .monitors
        .iter()
        .find(|monitor| monitor.name == left.name())
        .expect("a monitor that is off is not in the list a script is handed");
    assert!(row.off, "and the list does not say it is off");
    assert!(
        session.app.client.globals_removed.is_empty(),
        "a client was told a global went away: {:?}",
        session.app.client.globals_removed
    );

    session.power(&control, true);
    assert_eq!(
        tty.settle(&mut session.state, &left),
        [Step::Wake, Step::Draw],
        "back on is one frame drawn in full, and then frames as ever"
    );
    assert_eq!(layout(&session.state), before);
}

/// **The idle blank turns every screen off after the timeout** --
/// ten minutes when nothing says otherwise -- telling every
/// control, once per absence: a screen a client turns back on with
/// nobody there stays on. Zero never does it.
///
/// With the blank left out of `idle::settle`, fails at "still on";
/// with it not remembering that it has run, at "turned it off
/// again".
#[test]
fn the_idle_blank_turns_every_screen_off_after_the_timeout() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let controls = [session.control(0), session.control(1)];
    assert_eq!(
        session.state.idle.settings().screens_off_after,
        Duration::from_secs(600),
        "the default is not the ten minutes `idle::Settings` argues for"
    );

    // The first frame, which is where nobody-has-touched-it starts.
    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(599));
    assert!(
        session.none_off(&[&left, &right]),
        "a screen went off before the timeout"
    );
    session.wait(Duration::from_secs(2));
    assert!(
        session.all_off(&[&left, &right]),
        "ten minutes with nobody at the machine, and a screen is still on"
    );
    for control in &controls {
        assert_eq!(
            session.modes(control),
            [true, false],
            "and its control was not told"
        );
    }

    session.power(&controls[0], true);
    session.wait(Duration::from_secs(700));
    assert!(
        !session.state.power.is_off(&left),
        "a client turned a screen back on, and the idle blank turned it off \
         again with nobody having touched anything"
    );

    session.state.idle.configure(crate::idle::Settings {
        screens_off_after: Duration::ZERO,
        ..crate::idle::Settings::default()
    });
    session.state.power_all(true);
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Pressed, 1);
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Released, 2);
    session.wait(Duration::from_secs(10_000));
    assert!(
        session.none_off(&[&left, &right]),
        "idle.screens_off_after = 0 turned a screen off"
    );
}

/// **The idle lock locks the session after `idle.lock_after`** --
/// five minutes when nothing says otherwise -- with Solium's own
/// lock, as `lock.*` says by default. Once per absence: a session
/// unlocked from elsewhere with nobody at the machine is not locked
/// again at once. Input starts the count again, and zero never
/// does it.
///
/// With the lock left out of `idle::settle`, fails at "nobody
/// there"; with it not remembering that it has run, at "locked
/// again".
#[test]
fn the_idle_lock_locks_after_lock_after_once_per_absence() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    assert_eq!(
        session.state.idle.settings().lock_after,
        Duration::from_secs(300),
        "the default is not the five minutes `idle::Settings` argues for"
    );
    let natively_locked = |session: &Session| {
        session
            .state
            .lock
            .as_ref()
            .is_some_and(crate::lock::Lock::is_native)
    };
    // What `input::handle` does for every event from a device.
    let stir = |session: &mut Session| {
        let now = session.state.clock.now();
        session.state.idle.stir(now);
    };

    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(299));
    assert!(
        session.state.lock.is_none(),
        "locked before idle.lock_after"
    );
    session.wait(Duration::from_secs(2));
    assert!(
        natively_locked(&session),
        "five minutes with nobody there, and the session is not locked"
    );

    // logind's `Unlock`, from another console: nobody at this one.
    session.state.unlock_natively();
    session.wait(Duration::from_secs(1_000));
    assert!(
        session.state.lock.is_none(),
        "locked again with nobody having touched anything since"
    );

    stir(&mut session);
    session.wait(Duration::from_secs(299));
    assert!(
        session.state.lock.is_none(),
        "input did not start the count again"
    );
    session.wait(Duration::from_secs(2));
    assert!(natively_locked(&session), "a second absence was not locked");

    session.state.unlock_natively();
    session.state.idle.configure(crate::idle::Settings {
        lock_after: Duration::ZERO,
        ..crate::idle::Settings::default()
    });
    stir(&mut session);
    session.wait(Duration::from_secs(10_000));
    assert!(
        session.state.lock.is_none(),
        "idle.lock_after = 0 locked the session"
    );
}

/// **An idle inhibitor holds the idle lock off**, as it holds the
/// blank: a film is not locked half way through, and the count
/// starts from when it lets go.
#[test]
fn an_idle_inhibitor_holds_the_idle_lock_off() {
    let mut session = Session::new();
    let (_window, _toplevel, surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let film = session
        .app
        .client
        .inhibit
        .clone()
        .expect("zwp_idle_inhibit_manager_v1 bound")
        .create_inhibitor(&surface, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);

    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(3_600));
    assert!(
        session.state.lock.is_none(),
        "a film holding an idle inhibitor was locked"
    );

    film.destroy();
    session.app.pump(&mut session.display, &mut session.state);
    session.wait(Duration::from_secs(299));
    assert!(
        session.state.lock.is_none(),
        "locked before five minutes after the film ended"
    );
    session.wait(Duration::from_secs(2));
    assert!(
        session.state.lock.is_some(),
        "five minutes after the film ended, and not locked"
    );
}

/// **An idle inhibitor holds the idle blank off**, for as long as
/// it is held on a window that is on screen: a film does not go
/// dark. Let go, and the count starts again from then, as smithay's
/// and wlroots' do -- so the next film in a playlist, which takes
/// it again a few seconds later, does not start in the dark. Let go
/// for good, and the screen goes `screens_off_after` later. An
/// inhibitor taken and let go while the screen is off is an
/// absence of its own, and turns it off again at the end.
///
/// With `idle_inhibited` not asked, fails at "went dark"; counting
/// from the last input rather than from when it let go, at "the
/// moment the film ended"; with only input ending an absence, at
/// "an absence of its own".
#[test]
fn an_idle_inhibitor_holds_the_idle_blank_off() {
    let mut session = Session::new();
    session.without_the_idle_lock();
    let (_window, _toplevel, surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let monitor = session.output(0);
    let control = session.control(0);
    let manager = session
        .app
        .client
        .inhibit
        .clone()
        .expect("zwp_idle_inhibit_manager_v1 bound");
    let inhibit = |session: &mut Session| {
        let inhibitor = manager.create_inhibitor(&surface, &session.app.qh, ());
        session.app.pump(&mut session.display, &mut session.state);
        inhibitor
    };
    let release = |session: &mut Session, inhibitor: zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1| {
        inhibitor.destroy();
        session.app.pump(&mut session.display, &mut session.state);
    };

    let film = inhibit(&mut session);
    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(3_600));
    assert!(
        !session.state.power.is_off(&monitor),
        "a film holding an idle inhibitor went dark: an hour with nobody \
         touching anything is what watching one is"
    );

    release(&mut session, film);
    session.wait(Duration::from_secs(5));
    assert!(
        !session.state.power.is_off(&monitor),
        "the screen went dark the moment the film ended, an hour after \
         anybody touched anything"
    );
    let next = inhibit(&mut session);
    session.wait(Duration::from_secs(3_600));
    assert!(
        !session.state.power.is_off(&monitor),
        "the next film in the playlist went dark"
    );

    release(&mut session, next);
    session.wait(Duration::from_secs(599));
    assert!(
        !session.state.power.is_off(&monitor),
        "the screen went dark before ten minutes after the moment the film ended"
    );
    session.wait(Duration::from_secs(2));
    assert!(
        session.state.power.is_off(&monitor),
        "ten minutes after the last film ended and the screen is still lit, \
         so it was not the inhibitor holding it"
    );

    // A client turns it back on with nobody there, and something
    // holds the machine awake a while and lets go.
    session.power(&control, true);
    let call = inhibit(&mut session);
    session.wait(Duration::from_secs(60));
    release(&mut session, call);
    session.wait(Duration::from_secs(601));
    assert!(
        session.state.power.is_off(&monitor),
        "an inhibitor taken and let go was not an absence of its own: the \
         screen it was held on stayed lit"
    );
}

/// **An idle notification counts again from when an inhibitor
/// lets go**, as smithay's `IdleNotifierState::set_is_inhibited`
/// and wlroots do: `swayidle`'s `timeout 600 'wlopm --off \*'`
/// does not fire the moment a film an hour long ends. One that
/// ignores inhibitors -- `get_input_idle_notification`, a locker's
/// -- counts from the last input, whatever was held.
///
/// Counting from the last input, as before, fails at "the moment
/// the film ended".
#[test]
fn an_idle_notification_counts_again_from_when_an_inhibitor_lets_go() {
    let mut session = Session::new();
    let (_window, _toplevel, surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let notifier = session
        .app
        .client
        .idle_notifier
        .clone()
        .expect("ext_idle_notifier_v1 bound");
    let seat = session.app.client.seat.clone().expect("wl_seat bound");
    let film = session
        .app
        .client
        .inhibit
        .clone()
        .expect("zwp_idle_inhibit_manager_v1 bound")
        .create_inhibitor(&surface, &session.app.qh, ());
    let dims = notifier.get_idle_notification(600_000, &seat, &session.app.qh, ());
    let locks = notifier.get_input_idle_notification(1_800_000, &seat, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    let told = |session: &mut Session,
                notification: &ext_idle_notification_v1::ExtIdleNotificationV1| {
        session.app.pump(&mut session.display, &mut session.state);
        let id = wayland_client::Proxy::id(notification);
        session
            .app
            .client
            .idle_events
            .iter()
            .filter(|(each, _)| *each == id)
            .map(|(_, idled)| *idled)
            .collect::<Vec<_>>()
    };

    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(3_600));
    assert_eq!(
        told(&mut session, &dims),
        [false; 0],
        "held off by a film, and told it was idle"
    );
    assert_eq!(
        told(&mut session, &locks),
        [true],
        "a locker's notification was held off by a film"
    );

    film.destroy();
    session.app.pump(&mut session.display, &mut session.state);
    session.wait(Duration::from_secs(599));
    assert_eq!(
        told(&mut session, &dims),
        [false; 0],
        "told it was idle the moment the film ended, an hour after anybody \
         touched anything"
    );
    session.wait(Duration::from_secs(2));
    assert_eq!(
        told(&mut session, &dims),
        [true],
        "ten minutes after the film ended, and not told it was idle"
    );
}

/// A stand-in session bus with `org.freedesktop.ScreenSaver` owned
/// on it, as a nested run given `SOLIUM_SESSION_BUS` owns it.
fn owning_screensaver(session: &mut Session) -> crate::screensaver::tests::StandInBus {
    let bus = crate::screensaver::tests::StandInBus::new(false);
    session
        .state
        .idle
        .serve_dbus(crate::session::Place::Nested, Some(bus.address.clone()));
    assert_eq!(
        bus.requests().len(),
        1,
        "org.freedesktop.ScreenSaver was not asked for"
    );
    bus
}

/// **An inhibitor taken over D-Bus holds the idle blank off**, as
/// the `org.freedesktop.ScreenSaver.Inhibit` Chrome and Firefox
/// call during a film does, with no window of theirs anywhere,
/// and `UnInhibit` lets it happen `screens_off_after` later. An
/// `ext-idle-notify` notification that honours inhibitors --
/// `swayidle`'s timeouts -- is held off with it; a locker's, which
/// ignores them, is not.
///
/// With the D-Bus inhibitors not counted in `idle_inhibited`,
/// fails at "went dark"; with `UnInhibit` not heard, at "still
/// lit".
#[test]
fn a_dbus_inhibit_holds_the_idle_blank_off_and_uninhibit_lets_it_happen() {
    let mut session = Session::new();
    let monitor = session.output(0);
    let bus = owning_screensaver(&mut session);
    let notifier = session
        .app
        .client
        .idle_notifier
        .clone()
        .expect("ext_idle_notifier_v1 bound");
    let seat = session.app.client.seat.clone().expect("wl_seat bound");
    let dims = notifier.get_idle_notification(600_000, &seat, &session.app.qh, ());
    let locks = notifier.get_input_idle_notification(1_800_000, &seat, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    let told = |session: &mut Session,
                notification: &ext_idle_notification_v1::ExtIdleNotificationV1| {
        session.app.pump(&mut session.display, &mut session.state);
        let id = wayland_client::Proxy::id(notification);
        session
            .app
            .client
            .idle_events
            .iter()
            .filter(|(each, _)| *each == id)
            .map(|(_, idled)| *idled)
            .collect::<Vec<_>>()
    };

    bus.arrive(":1.42");
    let film = bus.inhibit(":1.42", "/org/freedesktop/ScreenSaver");
    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(3_600));
    assert!(
        !session.state.power.is_off(&monitor),
        "a film holding an inhibitor over D-Bus went dark"
    );
    assert_eq!(
        told(&mut session, &dims),
        [false; 0],
        "held off by a film over D-Bus, and swayidle was told it was idle"
    );
    assert_eq!(
        told(&mut session, &locks),
        [true],
        "a locker's notification was held off by a film"
    );

    bus.uninhibit(":1.42", film);
    session.wait(Duration::from_secs(599));
    assert!(
        !session.state.power.is_off(&monitor),
        "the screen went dark before ten minutes after the film let go"
    );
    session.wait(Duration::from_secs(2));
    assert!(
        session.state.power.is_off(&monitor),
        "ten minutes after UnInhibit and the screen is still lit"
    );
    assert_eq!(told(&mut session, &dims), [true]);
}

/// **Behind the lock screen a D-Bus inhibitor holds nothing**, as
/// a Wayland one holds nothing: a browser playing behind the lock
/// does not keep a lock screen lit all night.
///
/// With the D-Bus inhibitors counted before the lock is asked
/// about, fails at "stayed lit".
#[test]
fn a_dbus_inhibitor_holds_nothing_behind_the_lock_screen() {
    let mut session = Session::new();
    let monitor = session.output(0);
    let bus = owning_screensaver(&mut session);
    bus.arrive(":1.42");
    bus.inhibit(":1.42", "/ScreenSaver");
    let _lock = session.lock();

    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(601));
    assert!(
        session.state.power.is_off(&monitor),
        "behind the lock a browser's inhibitor held the lock screen lit"
    );
}

/// **`idle.dbus_inhibit = false` owns nothing**: the bus is not
/// even connected to. A reload that turns it on owns the name, and
/// one that turns it off again lets it go. A value that is not a
/// boolean keeps the default.
///
/// With the setting not read, fails at "connected"; with a reload
/// not heard, at "not asked for" or "kept".
#[test]
fn idle_dbus_inhibit_false_owns_nothing() {
    let mut session = Session::new();
    let directory =
        std::env::temp_dir().join(format!("solium-dbus-inhibit-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    let load = |session: &mut Session, lua: &str| {
        std::fs::write(&entry, lua).expect("writing the test script");
        session.state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        session.state.idle.settings().dbus_inhibit
    };
    assert!(
        load(&mut session, "sol.idle{ dbus_inhibit = 'no' }"),
        "a value that is not a boolean did not keep the default"
    );
    assert!(!load(&mut session, "sol.idle{ dbus_inhibit = false }"));

    let bus = crate::screensaver::tests::StandInBus::new(false);
    session
        .state
        .idle
        .serve_dbus(crate::session::Place::Nested, Some(bus.address.clone()));
    assert!(
        !bus.connected(Duration::from_millis(500)),
        "idle.dbus_inhibit = false connected to the bus"
    );

    assert!(load(&mut session, "sol.idle{}"));
    assert_eq!(
        bus.requests(),
        ["RequestName org.freedesktop.ScreenSaver 4"],
        "a reload turned it on and the name was not asked for"
    );
    assert!(!load(&mut session, "sol.idle{ dbus_inhibit = false }"));
    assert!(
        bus.closed(Duration::from_secs(5)),
        "a reload turned it off and the name was kept"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **Input wakes every screen**, and the input that wakes them is
/// delivered: a key press, a motion, a button. A release does not
/// wake them, because a binding that turns them off is let go
/// straight after.
///
/// Without the wake in `input::key`, fails at "a key press did not
/// wake"; waking on a release as well, at "a key let go"; without
/// the one in `input::handle`, at "the pointer moving".
#[test]
fn input_wakes_every_screen_the_idle_blank_turned_off() {
    let mut session = Session::with_monitors(2);
    session.without_the_idle_lock();
    session.app.open(&mut session.display, &mut session.state);
    let (left, right) = (session.output(0), session.output(1));
    let both = [&left, &right];

    // `a` goes down with the screens on -- a binding being pressed
    // -- and they go off while it is held.
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Pressed, 1);
    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(601));
    assert!(session.all_off(&both), "the premise: the idle blank ran");

    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Released, 2);
    assert!(
        session.all_off(&both),
        "a key let go woke the screens, so a binding that turns them off \
         turns them straight back on"
    );

    session.app.client.key_events.clear();
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Pressed, 3);
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Released, 4);
    assert!(
        session.none_off(&both),
        "a key press did not wake the screens"
    );
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session.app.client.key_events.contains(&(30, true)),
        "the key that woke the screens was not delivered: {:?}",
        session.app.client.key_events
    );

    let region = crate::monitor::union(&session.state.space).expect("two monitors");
    session.state.power_all(false);
    crate::synth::send_motion(&mut session.state, region, (4.0, 4.0).into(), 1_000);
    assert!(
        session.none_off(&both),
        "the pointer moving did not wake the screens"
    );

    session.state.power_all(false);
    crate::synth::send_button(
        &mut session.state,
        region,
        0x110,
        smithay::backend::input::ButtonState::Released,
        2_000,
    );
    assert!(session.all_off(&both), "a button let go woke the screens");
    crate::synth::send_button(
        &mut session.state,
        region,
        0x110,
        smithay::backend::input::ButtonState::Pressed,
        3_000,
    );
    assert!(
        session.none_off(&both),
        "a button pressed did not wake the screens"
    );
}

/// **A key that wakes a dark lock screen is typed into the lock
/// screen**: delivered, and to the lock client. The screen went dark
/// behind the lock with a film's inhibitor still held behind it,
/// because nothing holds it off while locked.
///
/// With the waking key swallowed, fails at "was swallowed"; with the
/// inhibitor counted while locked, at "stayed lit".
///
/// The press is what is asked about, not the key: a release goes
/// to whoever had the press, and `type_key` sends both.
#[test]
fn a_key_that_wakes_a_dark_lock_screen_is_typed_into_the_lock_screen() {
    let mut session = Session::new();
    let (_window, _toplevel, surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let _film = session
        .app
        .client
        .inhibit
        .clone()
        .expect("zwp_idle_inhibit_manager_v1 bound")
        .create_inhibitor(&surface, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    let monitor = session.output(0);
    let lock = session.lock();
    let mut tty = Tty::default();

    session.wait(Duration::ZERO);
    session.wait(Duration::from_secs(601));
    assert!(
        session.state.power.is_off(&monitor),
        "behind the lock the film's inhibitor holds nothing, and the lock \
         screen stayed lit"
    );
    assert_eq!(
        tty.settle(&mut session.state, &monitor),
        [Step::Blank, Step::Darken, Step::Rest]
    );

    session.locker.client.key_events.clear();
    let (app, _) = session.type_key();
    assert!(
        !app,
        "the key that woke the lock screen reached the application behind it"
    );
    assert!(
        session.locker.client.key_events.contains(&(30, true)),
        "the key that woke the lock screen was swallowed: the first key of a \
         password typed at a dark screen is lost ({:?})",
        session.locker.client.key_events
    );
    assert!(
        !session.state.power.is_off(&monitor),
        "a key typed at a dark lock screen did not light it"
    );
    assert_eq!(tty.frame(&mut session.state, &monitor), Step::Wake);
    session.assert_unlocks(lock);
}

/// **A dark monitor does not hold `locked` back**: it shows
/// nothing, so there is nothing on it to cover. `locked` still
/// waits for the monitor that is on.
///
/// With a dark monitor waited for like any other, fails at "waits
/// for the dark one".
#[test]
fn a_dark_monitor_does_not_hold_locked_back() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let mut tty = Tty::default();
    let control = session.control(1);
    session.power(&control, false);
    assert_eq!(
        tty.settle(&mut session.state, &right),
        [Step::Blank, Step::Darken, Step::Rest]
    );

    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    assert!(
        !session.told_locked(&lock),
        "the lock client was told `locked` before the monitor that is on \
         had shown the lock"
    );
    tty.frame(&mut session.state, &left);
    assert!(
        session.told_locked(&lock),
        "the monitor that is on showed the lock, and `locked` waits for the \
         dark one"
    );
}

/// **A monitor asked off but not yet dark still holds `locked`
/// back.** Its black frame was queued before the lock and has not
/// flipped, so it is still showing what it had -- the desktop --
/// and the flip of a frame built before the lock counts for
/// nothing. Only once it is dark is it not waited for.
///
/// With `confirm_lock` counting a monitor asked off rather than one
/// that is dark, fails at "still showing".
#[test]
fn a_monitor_asked_off_but_not_yet_dark_still_holds_locked_back() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let mut tty = Tty::default();
    let control = session.control(1);
    session.power(&control, false);
    assert_eq!(tty.queue(&mut session.state, &right), Step::Blank);

    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    tty.frame(&mut session.state, &left);
    assert!(
        !session.told_locked(&lock),
        "the lock client was told `locked` while the right-hand monitor was \
         still showing the desktop, its black frame not yet flipped"
    );
    tty.flip(&mut session.state, &right);
    assert!(
        !session.told_locked(&lock),
        "the flip of a black frame queued before the lock counted as the lock"
    );
    assert_eq!(tty.frame(&mut session.state, &right), Step::Darken);
    assert!(
        session.told_locked(&lock),
        "the right-hand monitor is dark and `locked` waits for it"
    );
}

/// **A monitor powered off while `locked` waits lets it go**, and
/// no later than it would have gone: `swayidle` turning the screens
/// off while the locker is still starting must not delay the lock.
///
/// Two ways, either of which is enough: its black frame is built
/// under the lock, as `tty.rs`'s `blank` builds it, so that flip
/// already counts; and once dark it is not waited for. With both
/// taken away -- a dark monitor waited for, and a black frame
/// counting for nothing -- fails at "is still waiting".
#[test]
fn a_monitor_powered_off_while_locking_lets_locked_go() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let mut tty = Tty::default();
    let control = session.control(1);
    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    tty.frame(&mut session.state, &left);
    assert!(!session.told_locked(&lock), "the premise");

    session.power(&control, false);
    tty.settle(&mut session.state, &right);
    assert!(
        session.told_locked(&lock),
        "the right-hand monitor went dark and the lock client is still waiting"
    );
}

/// **A monitor powered on while `locked` waits must show the lock
/// too**: dark, it was not waited for; on, it is a screen like the
/// others. Its first frame is built under the lock.
///
/// With a monitor that was dark still counted once it is on, fails at
/// "had shown nothing".
#[test]
fn a_monitor_powered_on_while_locking_must_show_the_lock_too() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let mut tty = Tty::default();
    let control = session.control(1);
    session.power(&control, false);
    tty.settle(&mut session.state, &right);

    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    session.power(&control, true);
    tty.frame(&mut session.state, &left);
    assert!(
        !session.told_locked(&lock),
        "the lock client was told `locked` while the monitor powered on had \
         shown nothing of the lock"
    );
    assert_eq!(tty.frame(&mut session.state, &right), Step::Wake);
    assert!(
        session.told_locked(&lock),
        "both monitors showed the lock and the lock client was not told"
    );
}

/// **Powering a monitor on while locked draws only the lock.** The
/// monitor went dark before the lock, and what its display was left
/// holding is the black frame, never the desktop. Powered on behind
/// the lock -- `swayidle`'s `resume` -- its first frame is built
/// under the lock, which `render::elements` draws as the lock and
/// its backdrop and nothing else
/// (`the_drag_icon_is_emitted_below_the_lock_screens_early_return`),
/// and drawn in full, so no part of it comes from a buffer drawn
/// before the lock.
///
/// With going off switching the display off first and drawing black
/// after, fails at "left holding"; with a wake not drawn in full, at
/// "in full".
#[test]
fn powering_a_monitor_on_while_locked_draws_only_the_lock() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let selections = session.app.client.selections;
    let monitor = session.output(0);
    let mut tty = Tty::default();
    let control = session.control(0);
    session.power(&control, false);
    assert_eq!(
        tty.settle(&mut session.state, &monitor),
        [Step::Blank, Step::Darken, Step::Rest],
        "what the display was left holding is not the black frame"
    );

    let (lock, surfaces) = session
        .locker
        .ask_lock(&mut session.display, &mut session.state, true);
    session.lock_surfaces = surfaces;
    assert!(
        session.told_locked(&lock),
        "the only monitor is dark, so there is nothing for `locked` to wait \
         for, and it waited"
    );

    session.power(&control, true);
    let built_under = session.state.lock_frame();
    assert!(
        built_under.is_some(),
        "the first frame back is not built under the lock: it is the desktop"
    );
    assert_eq!(
        tty.frame(&mut session.state, &monitor),
        Step::Wake,
        "the first frame back is not drawn in full"
    );
    assert_eq!(tty.frame(&mut session.state, &monitor), Step::Draw);
    session.assert_sealed("a monitor powered on while locked", selections);
    session.assert_unlocks(lock);
}

/// **A monitor that is off cannot be captured**: a capture asked
/// for is refused, and so is one asked for while it was on and
/// copied after, and one already waiting for its next frame. A
/// screenshot tool is told no rather than left waiting.
///
/// Without the checks in `screencopy.rs`, fails at "was offered".
#[test]
fn an_off_monitor_cannot_be_captured() {
    let mut session = Session::new();
    session.app.open(&mut session.display, &mut session.state);
    let manager = session
        .app
        .client
        .screencopy
        .clone()
        .expect("zwlr_screencopy_manager_v1 bound");
    let output = session.app.client.output.clone().expect("wl_output bound");
    let buffer = |session: &mut Session| {
        let shm = session.app.client.shm.clone().expect("wl_shm bound");
        let fd = anon_file(64 * 64 * 4);
        let pool = shm.create_pool(fd.as_fd(), 64 * 64 * 4, &session.app.qh, ());
        pool.create_buffer(
            0,
            64,
            64,
            64 * 4,
            wl_shm::Format::Xrgb8888,
            &session.app.qh,
            (),
        )
    };
    let before = manager.capture_output(0, &output, &session.app.qh, ());
    let waiting = manager.capture_output(0, &output, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    let waiting_buffer = buffer(&mut session);
    waiting.copy(&waiting_buffer);
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(
        session.app.client.captures_offered, 2,
        "with the monitor on, a capture was not offered a buffer, so refusing \
         one proves nothing"
    );
    assert_eq!(session.state.pending_captures.len(), 1, "the premise");

    let control = session.control(0);
    session.power(&control, false);
    assert_eq!(
        session.app.client.captures_failed, 1,
        "a capture waiting for the next frame of a monitor that went off was \
         left waiting"
    );

    let _during = manager.capture_output(0, &output, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(
        session.app.client.captures_offered, 2,
        "a monitor that is off was offered for capture"
    );
    assert_eq!(
        session.app.client.captures_failed, 2,
        "and nobody was told no"
    );

    let before_buffer = buffer(&mut session);
    before.copy(&before_buffer);
    session.app.pump(&mut session.display, &mut session.state);
    assert_eq!(
        session.app.client.captures_failed, 3,
        "a capture asked for while it was on and copied after was not refused"
    );
}

/// **A window being resized is told it may draw again as soon as a
/// frame showing it starts to be drawn** (`resize.overlap`), not
/// only once that frame reaches the screen, so its application
/// draws its next size while the compositor draws its last. A
/// window nobody is resizing is not told then -- it waits for the
/// refresh, as before -- and neither is anything with the setting
/// off.
#[test]
fn a_window_being_resized_is_told_to_draw_again_when_its_frame_starts() {
    let mut session = Session::with_monitors(1);
    let (window, _toplevel, surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let (_other, _toplevel, other, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let output = session.output(0);
    session.state.space.refresh();
    let ask = |session: &mut Session, surface: &wl_surface::WlSurface| {
        surface.frame(&session.app.qh, FrameDone);
        surface.commit();
        session.app.pump(&mut session.display, &mut session.state);
    };
    let done = |session: &mut Session| {
        session.app.pump(&mut session.display, &mut session.state);
        session.app.client.frames_done
    };
    let refresh = Duration::from_millis(16);

    ask(&mut session, &surface);
    ask(&mut session, &other);
    session
        .state
        .send_resize_frames_on(&output, Duration::ZERO, refresh);
    assert_eq!(done(&mut session), 0, "nobody is being resized");

    let pane = session
        .state
        .panes
        .id_of(&window)
        .expect("the window has a pane");
    let size = window.geometry().size;
    session.state.resize_hold = Some(crate::resizing::Held {
        window: window.clone(),
        pane,
        hold: crate::resizing::Hold::new(
            ResizeEdge::BottomRight,
            size,
            Rectangle::new((0, 0).into(), size),
            Duration::ZERO,
            None,
            crate::resizing::PATIENCE,
            crate::resizing::EVERY_FRAME,
        ),
    });
    session
        .state
        .send_resize_frames_on(&output, Duration::ZERO, refresh);
    assert_eq!(
        done(&mut session),
        1,
        "the window being resized is told, and the other is not"
    );

    ask(&mut session, &surface);
    session.state.resizing.overlap = false;
    session
        .state
        .send_resize_frames_on(&output, Duration::ZERO, refresh);
    assert_eq!(
        done(&mut session),
        1,
        "and with resize.overlap off, nobody is"
    );
}

/// **A window on a dark monitor is told to draw once a second, and
/// not every frame.** A lit monitor's refresh tells it nothing any
/// more; the dark pass tells it at most once per
/// `idle.off_frame_interval`; and 0 tells it nothing at all, as sway
/// does. See "Frame callbacks on a dark monitor" in `power.rs`.
///
/// With a lit monitor's refresh still telling every window, fails
/// at "the right-hand monitor's refresh"; with the dark pass not
/// throttled, at "more than once a second".
#[test]
fn a_window_on_a_dark_monitor_is_told_to_draw_once_a_second_and_not_every_frame() {
    let mut session = Session::with_monitors(2);
    let (_window, _toplevel, surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let (left, right) = (session.output(0), session.output(1));
    session.state.space.refresh();
    let ask = |session: &mut Session| {
        surface.frame(&session.app.qh, FrameDone);
        surface.commit();
        session.app.pump(&mut session.display, &mut session.state);
    };
    let at = Duration::from_millis;
    let refresh = Duration::from_millis(16);
    let done = |session: &mut Session| {
        session.app.pump(&mut session.display, &mut session.state);
        session.app.client.frames_done
    };

    ask(&mut session);
    session.state.send_frames_on(&left, at(0), refresh);
    assert_eq!(
        done(&mut session),
        1,
        "lit, its monitor's refresh told it to draw"
    );

    let control = session.control(0);
    session.power(&control, false);
    ask(&mut session);
    session.state.send_frames_on(&right, at(1_100), refresh);
    assert_eq!(
        done(&mut session),
        1,
        "the right-hand monitor's refresh told a window only the dark \
         left-hand one shows to draw"
    );
    session.state.send_dark_frames(at(1_100));
    assert_eq!(
        done(&mut session),
        2,
        "a second on, the dark pass did not tell it"
    );

    ask(&mut session);
    for step in 1..60 {
        session.state.send_dark_frames(at(1_100 + step * 16));
    }
    assert_eq!(
        done(&mut session),
        2,
        "told to draw more than once a second on a monitor that is off"
    );
    session.state.send_dark_frames(at(2_150));
    assert_eq!(done(&mut session), 3, "and not at all a second later");

    session.state.idle.configure(crate::idle::Settings {
        off_frame_interval: Duration::ZERO,
        ..crate::idle::Settings::default()
    });
    ask(&mut session);
    session.state.send_dark_frames(at(60_000));
    assert_eq!(
        done(&mut session),
        3,
        "idle.off_frame_interval = 0 still told it to draw"
    );
}

/// **`sol.monitor_power` turns a monitor off and on, and
/// `sol.monitors()` says which are off**: every one, or one by
/// name. A mode that is neither `"on"` nor `"off"` is an error and
/// changes nothing, and so does a name no monitor has. `sol.idle`
/// reaches the compositor.
///
/// Without `power` in `sol.monitors()`, fails at "whether a monitor
/// is on"; with the snapshot not saying so, at "does not say they
/// are off".
#[test]
fn sol_monitor_power_turns_a_monitor_off_and_on_and_says_so() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let control = session.control(1);

    let directory =
        std::env::temp_dir().join(format!("solium-monitor-power-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            r#"
            sol.idle{{ lock_after = 45, screens_off_after = 90, off_frame_interval = 250 }}
            sol.bind("super+o", function() sol.monitor_power("all", "off") end)
            sol.bind("super+p", function() sol.monitor_power({left:?}, "on") end)
            sol.bind("super+i", function() sol.monitor_power("all", "dim") end)
            sol.bind("super+u", function() sol.monitor_power("nowhere", "on") end)
            "#,
            left = left.name(),
        ),
    )
    .expect("writing the test script");
    session.state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));
    let _ = std::fs::remove_dir_all(&directory);

    assert_eq!(
        session.state.idle.settings(),
        crate::idle::Settings {
            lock_after: Duration::from_secs(45),
            screens_off_after: Duration::from_secs(90),
            off_frame_interval: Duration::from_millis(250),
            dbus_inhibit: true,
        },
        "sol.idle did not reach the compositor"
    );
    let said = |session: &Session| {
        session
            .state
            .scripts
            .as_ref()
            .map(|scripts| {
                scripts.evaluate_in(
                    session.state.snapshot(),
                    "local m = sol.monitors() return m[1].power .. ' ' .. m[2].power",
                )
            })
            .unwrap_or_default()
    };
    assert_eq!(
        said(&session),
        "on on",
        "sol.monitors() does not say whether a monitor is on"
    );

    assert!(session.state.trigger("super+o"), "the binding did not run");
    assert!(
        session.all_off(&[&left, &right]),
        "\"all\", \"off\" left a monitor on"
    );
    assert_eq!(
        session.modes(&control),
        [true, false],
        "a client watching the right-hand monitor was not told it went off"
    );
    assert_eq!(
        said(&session),
        "off off",
        "sol.monitors() does not say they are off"
    );

    session.state.trigger("super+p");
    assert_eq!(said(&session), "on off", "one monitor, by name");

    session.state.trigger("super+i");
    session.state.trigger("super+u");
    assert_eq!(
        said(&session),
        "on off",
        "a mode that is not on or off, or a monitor that is not there, changed \
         something"
    );
    assert!(!session.state.power.is_off(&left) && session.state.power.is_off(&right));
}

/// **A binding that turns the screens off leaves them off when it
/// is let go.** The press wakes nothing -- they are on -- and then
/// runs the binding; the release is not a wake. So pressing it in
/// the dark does the same: the press wakes them, and the binding
/// turns them straight off again.
///
/// With the wake after the filter rather than before it, fails at
/// "the key that ran it"; waking on a release, at "was let go".
#[test]
fn a_binding_that_turns_the_screens_off_leaves_them_off_when_it_is_let_go() {
    let mut session = Session::new();
    let monitor = session.output(0);
    let directory =
        std::env::temp_dir().join(format!("solium-power-binding-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        r#"sol.bind("a", function() sol.monitor_power("all", "off") end)"#,
    )
    .expect("writing the test script");
    session.state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));
    let _ = std::fs::remove_dir_all(&directory);

    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Pressed, 1);
    assert!(
        session.state.power.is_off(&monitor),
        "the binding ran and the screen is on: the key that ran it woke it again"
    );
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Released, 2);
    assert!(
        session.state.power.is_off(&monitor),
        "the screen came back on when the binding that turned it off was let go"
    );

    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Pressed, 3);
    crate::input::key(&mut session.state, Keycode::new(38), KeyState::Released, 4);
    assert!(
        session.state.power.is_off(&monitor),
        "pressed in the dark, and the binding did not turn the screen back off"
    );
}

/// **A display that will not switch off is `failed`, and stays
/// on**: what `tty.rs`'s `darken` does when `DrmCompositor::clear`
/// errs, played through the call it makes. The control is told
/// `failed`, stops being one, and asks for nothing afterwards; and
/// a backend saying a monitor that is on is dark is not believed.
/// The DRM half needs a GPU, and no test reaches it.
#[test]
fn a_display_that_will_not_switch_off_is_failed_and_stays_on() {
    let mut session = Session::new();
    let monitor = session.output(0);
    let control = session.control(0);
    session.power(&control, false);
    session.state.power_refused(&monitor);
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        session
            .app
            .client
            .power_failed
            .contains(&wayland_client::Proxy::id(&control)),
        "a monitor that could not be switched off was not answered `failed`"
    );
    assert!(
        !session.state.power.is_off(&monitor),
        "and it is not on again"
    );

    session.power(&control, false);
    assert!(
        !session.state.power.is_off(&monitor),
        "a control told `failed` still turned its monitor off"
    );

    session.state.output_dark(&monitor);
    assert!(
        !session.state.power.is_dark(&monitor),
        "a monitor that is on was counted dark, which would let a lock go \
         past it"
    );
}
