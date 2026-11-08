/// `wp_fifo_v1` and `wp_commit_timing_v1` (#76), played on a panel: a layer
/// surface, which a refresh of its monitor paces like a window. See
/// `fifo.rs`. Each commit marks itself with an opaque region as wide as its
/// number, which says whether it has been applied: it is state a held commit
/// carries, and nothing else changes it here.
mod client_pacing {
    use super::*;
    use wayland_protocols::wp::{
        commit_timing::v1::client::wp_commit_timer_v1::WpCommitTimerV1,
        fifo::v1::client::wp_fifo_v1::WpFifoV1,
    };

    /// The panel's own surface as the server has it.
    fn panel_surface(monitor: &Output) -> WlSurface {
        smithay::desktop::layer_map_for_output(monitor)
            .layers()
            .next()
            .map(|layer| layer.wl_surface().clone())
            .expect("the panel is on the monitor")
    }

    /// The mark of the last commit applied to `surface`, 1 for none.
    fn applied(surface: &WlSurface) -> i32 {
        smithay::wayland::compositor::with_states(surface, |states| {
            states
                .cached_state
                .get::<smithay::wayland::compositor::SurfaceAttributes>()
                .current()
                .opaque_region
                .as_ref()
                .and_then(|region| region.rects.first())
                .map_or(1, |(_, rect)| rect.size.w)
        })
    }

    /// Mark the next commit of `surface` as `number`.
    fn mark(session: &Session, surface: &wl_surface::WlSurface, number: i32) {
        let region = session
            .app
            .client
            .compositor
            .clone()
            .expect("wl_compositor bound")
            .create_region(&session.app.qh, ());
        region.add(0, 0, number, 1);
        surface.set_opaque_region(Some(&region));
        region.destroy();
    }

    /// A panel with a fifo object, whose first commit has set a barrier.
    fn fifo_panel(session: &mut Session) -> (Panel, WpFifoV1) {
        let panel = session
            .app
            .panel(&mut session.display, &mut session.state, 0);
        let fifo = session
            .app
            .client
            .fifo
            .clone()
            .expect("wp_fifo_manager_v1 is advertised")
            .get_fifo(&panel.surfaces[0], &session.app.qh, ());
        fifo.set_barrier();
        panel.surfaces[0].commit();
        session.app.pump(&mut session.display, &mut session.state);
        (panel, fifo)
    }

    /// The next frame, marked `number`: waits on the barrier the last one
    /// set, and sets its own.
    fn queue_frame(session: &mut Session, panel: &Panel, fifo: &WpFifoV1, number: i32) {
        fifo.wait_barrier();
        fifo.set_barrier();
        mark(session, &panel.surfaces[0], number);
        panel.surfaces[0].commit();
        session.app.pump(&mut session.display, &mut session.state);
    }

    /// A frame of `monitor` drawn now and then flipped, as a backend plays
    /// it: every commit applied so far is in it.
    fn refresh(session: &mut Session, monitor: &Output, time: Duration) {
        crate::fifo::latch(monitor, crate::pacing::monotonic_now());
        session.state.send_frames_on(monitor, time, REFRESH);
        session.app.pump(&mut session.display, &mut session.state);
    }

    /// `at` on the presentation clock, as a client sends it.
    fn stamp(timer: &WpCommitTimerV1, at: Duration) {
        let seconds = at.as_secs();
        timer.set_timestamp((seconds >> 32) as u32, seconds as u32, at.subsec_nanos());
    }

    /// **A commit that waits on a fifo barrier is applied once its
    /// monitor has refreshed, and not before**: what a Vulkan swapchain
    /// in FIFO mode relies on to have every frame it queues shown.
    ///
    /// Without the global, fails at "is advertised"; with barriers never
    /// cleared by a refresh, at "a refresh did not".
    #[test]
    fn a_commit_waiting_on_a_fifo_barrier_is_applied_after_its_monitor_refreshes() {
        let mut session = Session::new();
        let monitor = session.output(0);
        let (panel, fifo) = fifo_panel(&mut session);
        let surface = panel_surface(&monitor);

        queue_frame(&mut session, &panel, &fifo, 2);
        assert_eq!(
            applied(&surface),
            1,
            "applied while the frame before it had not been shown for a refresh"
        );

        refresh(&mut session, &monitor, at(0));
        assert_eq!(
            applied(&surface),
            2,
            "a refresh did not let the waiting commit go: a FIFO swapchain stalls"
        );

        queue_frame(&mut session, &panel, &fifo, 3);
        assert_eq!(
            applied(&surface),
            2,
            "the next frame did not wait for the barrier its predecessor set"
        );
        refresh(&mut session, &monitor, at(16));
        assert_eq!(applied(&surface), 3, "one frame a refresh");
    }

    /// **A refresh clears only a barrier the frame it shows has**: a commit
    /// applied after that frame was drawn, which on the hardware is the
    /// refresh before its flip, waits for the next. Cleared at that flip,
    /// the next frame would replace it before any frame showed it.
    ///
    /// With every refresh clearing the barrier now set, fails at "a frame
    /// that did not show".
    #[test]
    fn a_commit_applied_after_its_monitors_frame_was_drawn_waits_for_the_next() {
        let mut session = Session::new();
        let monitor = session.output(0);
        let panel = session
            .app
            .panel(&mut session.display, &mut session.state, 0);
        let surface = panel_surface(&monitor);
        let fifo = session
            .app
            .client
            .fifo
            .clone()
            .expect("wp_fifo_manager_v1 is advertised")
            .get_fifo(&panel.surfaces[0], &session.app.qh, ());

        // Drawn, and then the commit that sets the barrier lands before the
        // frame flips.
        crate::fifo::latch(&monitor, crate::pacing::monotonic_now());
        fifo.set_barrier();
        panel.surfaces[0].commit();
        session.app.pump(&mut session.display, &mut session.state);
        queue_frame(&mut session, &panel, &fifo, 2);

        session.state.send_frames_on(&monitor, at(0), REFRESH);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            1,
            "let go by the flip of a frame that did not show the commit it waited on"
        );
        refresh(&mut session, &monitor, at(16));
        assert_eq!(applied(&surface), 2, "and not by the next, which did");
    }

    /// **A fifo wait no refresh reaches is let go after
    /// `idle.off_frame_interval`**, the cadence a surface no lit screen
    /// shows is told it may draw at, and **0 holds it**, as it holds
    /// those frame callbacks.
    ///
    /// Without the forward-progress check, fails at "held for ever"; with
    /// it ignoring the setting, at "0 still let it go".
    #[test]
    fn a_fifo_wait_no_refresh_reaches_is_let_go_after_the_off_frame_interval() {
        let mut session = Session::new();
        let monitor = session.output(0);
        let (panel, fifo) = fifo_panel(&mut session);
        let surface = panel_surface(&monitor);
        let interval = session.state.idle.settings().off_frame_interval;
        assert_eq!(
            interval,
            Duration::from_secs(1),
            "the premise: the shipped default"
        );

        queue_frame(&mut session, &panel, &fifo, 2);
        let committed = crate::pacing::monotonic_now();
        crate::fifo::settle_at(&mut session.state, committed + interval / 2);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            1,
            "let go before idle.off_frame_interval had passed"
        );
        crate::fifo::settle_at(&mut session.state, committed + interval * 2);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            2,
            "held for ever by a barrier no refresh reached"
        );

        session.state.idle.configure(crate::idle::Settings {
            off_frame_interval: Duration::ZERO,
            ..crate::idle::Settings::default()
        });
        queue_frame(&mut session, &panel, &fifo, 3);
        crate::fifo::settle_at(
            &mut session.state,
            crate::pacing::monotonic_now() + Duration::from_secs(60),
        );
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            2,
            "idle.off_frame_interval = 0 still let it go"
        );
        refresh(&mut session, &monitor, at(0));
        assert_eq!(applied(&surface), 3, "and a refresh still does");
    }

    /// **A commit timed for later is applied when the frame drawn next
    /// would reach the screen no earlier than its time**: at its time on a
    /// screen that draws nothing, and on the hardware by the flip before
    /// it, so it is shown at the vblank it named, never at one before.
    ///
    /// Without the global, fails at "is advertised"; with timed commits
    /// cleared only by a refresh, at "a still screen"; with every refresh
    /// clearing them, at "ten seconds early"; with a refresh from now taken
    /// for the next frame where no vblank is known, at "are not known".
    #[test]
    fn a_timed_commit_is_applied_no_earlier_than_its_time() {
        let mut session = Session::new();
        let monitor = session.output(0);
        let panel = session
            .app
            .panel(&mut session.display, &mut session.state, 0);
        let surface = panel_surface(&monitor);
        let timer = session
            .app
            .client
            .commit_timing
            .clone()
            .expect("wp_commit_timing_manager_v1 is advertised")
            .get_timer(&panel.surfaces[0], &session.app.qh, ());

        let due = crate::pacing::monotonic_now() + Duration::from_secs(10);
        stamp(&timer, due);
        mark(&session, &panel.surfaces[0], 2);
        panel.surfaces[0].commit();
        session.app.pump(&mut session.display, &mut session.state);
        session.state.send_frames_on(&monitor, at(0), REFRESH);
        crate::fifo::settle(&mut session.state);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            1,
            "applied ten seconds early, by a refresh and a settle long before its time"
        );
        crate::fifo::settle_at(&mut session.state, due);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            2,
            "not applied at its time on a still screen that draws nothing"
        );

        // Nested, where no vblank is known: due in a quarter of a refresh,
        // held by a refresh now, and let go at its time.
        let due = crate::pacing::monotonic_now() + REFRESH / 4;
        stamp(&timer, due);
        mark(&session, &panel.surfaces[0], 3);
        panel.surfaces[0].commit();
        session.app.pump(&mut session.display, &mut session.state);
        session.state.send_frames_on(&monitor, at(16), REFRESH);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            2,
            "let go before its time by a refresh of a monitor whose vblanks are not known"
        );
        crate::fifo::settle_at(&mut session.state, due);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(applied(&surface), 3, "not let go at its time");

        // On the hardware, a monitor that has just flipped: the frame drawn
        // next is on the screen a refresh after the flip. Due before then,
        // this refresh lets it go; due after, it does not.
        let flip = crate::pacing::monotonic_now();
        crate::fifo::flipped(&monitor, flip);
        stamp(&timer, flip + REFRESH * 3 / 2);
        mark(&session, &panel.surfaces[0], 4);
        panel.surfaces[0].commit();
        session.app.pump(&mut session.display, &mut session.state);
        session.state.send_frames_on(&monitor, at(32), REFRESH);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            3,
            "let go by a flip whose next frame reaches the screen before its time"
        );
        crate::fifo::settle_at(&mut session.state, flip + REFRESH * 3 / 2);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(applied(&surface), 4, "the premise: let go at its time");

        let flip = crate::pacing::monotonic_now();
        crate::fifo::flipped(&monitor, flip);
        stamp(&timer, flip + REFRESH * 3 / 4);
        mark(&session, &panel.surfaces[0], 5);
        panel.surfaces[0].commit();
        session.app.pump(&mut session.display, &mut session.state);
        session.state.send_frames_on(&monitor, at(48), REFRESH);
        session.app.pump(&mut session.display, &mut session.state);
        assert_eq!(
            applied(&surface),
            5,
            "a flip whose next frame reaches the screen after its time held it a refresh late"
        );
    }
}
