//! Turning a monitor off without taking it away: `zwlr_output_power_manager_v1`,
//! `sol.monitor_power`, and the idle blank in `idle.rs`.
//!
//! **Off is not gone.** A monitor that is off keeps its `wl_output`, its place
//! in the arrangement, its work area and every window on it; only the display
//! stops showing anything. A client is never told the screen went away, a
//! layout is never asked to move anything, and turning it back on changes
//! nothing but what is lit
//! (`an_off_monitor_is_not_drawn_and_nothing_about_the_layout_changes`). That
//! is the difference from `enabled = false` in the configuration, which does
//! take the monitor out and frees its CRTC.
//!
//! Three things can ask, and they all end in [`Solium::set_power`]: a client
//! over the protocol (`wlopm`, and `swayidle` driving it), a script with
//! `sol.monitor_power`, and the idle blank. One thing turns every screen back
//! on without asking: input. See [`Solium::wake_screens`].
//!
//! ## What "off" is, per backend
//!
//! A backend asks [`Solium::power_step`] what to do with each monitor as it
//! draws, and answers with the monitor's [`Lit`] -- what the backend has done
//! to it so far. The answer is [`step`], which is the part with the reasoning
//! in it and the part a test can reach
//! (`every_way_a_monitor_goes_off_and_on_is_one_step_at_a_time`).
//!
//! * **tty.** One frame of black, then `DrmCompositor::clear` once that frame
//!   has flipped. `clear` is smithay 0.7's own power-off: "Clear the surface,
//!   setting DPMS state to off, disabling all planes, and clearing the pending
//!   frame. Calling `queue_frame` will re-enable." -- on an atomic device it
//!   commits the CRTC and its connectors off with every plane reset
//!   (`AtomicDrmSurface::clear_state`), on a legacy one it sets the connectors'
//!   DPMS property off. Turning it back on is the next frame: the surface's
//!   *pending* state still holds the mode and the connectors, so the commit
//!   that queues it is a modeset back to exactly the mode it had, and the
//!   buffers are reset first so that frame is drawn in full. The black frame
//!   is for the legacy path, which leaves the last framebuffer attached to the
//!   CRTC and turns DPMS back on *before* flipping the next one: what the
//!   monitor could show for that one refresh is black, never whatever was on
//!   it when it went off -- which may be a desktop that has since been locked.
//!   The `power` tests in `state/tests.rs` play this sequence through the
//!   calls `tty.rs` makes; the DRM calls themselves need a GPU, and no test
//!   reaches them.
//! * **nested.** There is no display to turn off, and answering `failed` would
//!   make the whole feature -- the protocol, the idle blank, `sol.monitor_power`
//!   and what the lock does with a dark monitor -- impossible to try anywhere
//!   but a tty. So that monitor's share of the window is drawn black once and
//!   then not drawn at all, the client is told `off`, and the log says it was
//!   blanked rather than powered off. No test reaches the nested loop.
//! * **tests.** No renderer: they play a backend through the same two calls.
//!
//! `failed` is sent for a monitor that is not there -- one unplugged, or a
//! `wl_output` for one that has gone -- and by a tty that could not clear the
//! CRTC (`Solium::power_refused`,
//! `a_display_that_will_not_switch_off_is_failed_and_stays_on`). There is no
//! exclusive control: wlroots fails a second control for the same output,
//! which a protocol allows and which would mean a panel holding one stops
//! `wlopm` working. Every control is told every change instead
//! (`the_power_protocol_is_advertised_answers_mode_on_bind_and_turns_a_monitor_off_and_on`).
//!
//! ## The lock
//!
//! A dark monitor shows nothing, so it has nothing for a lock to cover, and it
//! counts as having shown the lock (`a_dark_monitor_does_not_hold_locked_back`).
//! *Dark*, not *asked to be off*: until the backend says it has cleared it, a
//! monitor is still scanning out whatever it had, and that may be the desktop.
//! A monitor asked back on stops counting at once, so a lock still waiting
//! waits for it to show the lock like any other. See `Solium::confirm_lock`.
//!
//! ## Frame callbacks on a dark monitor
//!
//! **Throttled, to one a second by default (`idle.off_frame_interval`), not
//! stopped.** The two compositors worth comparing disagree:
//!
//! * [sway][sway-frame] sends none. `handle_frame` returns before
//!   `send_frame_done_iterator` for an output that is not enabled, and a
//!   powered-off `wlr_output` produces no frame events to begin with.
//! * [niri][niri-fallback] sends one about a second: a timer every second
//!   sends a frame callback to every window whose last one is more than 995 ms
//!   old, and it is the only thing that does while its monitors are off,
//!   because [the render][niri-redraw] is skipped and the per-output sequence
//!   only advances when a frame is [queued][niri-queue].
//!
//! Solium takes niri's side because of what "none" means here. With every
//! screen off there is no vblank at all, so under "none" every client that
//! waits for its callback inside `eglSwapBuffers` -- Mesa's FIFO path does --
//! is frozen mid-swap for as long as the screens are dark, which with the idle
//! blank is all night. Its whole thread stops, not only its drawing. One
//! wake-up a second per window costs nothing and keeps them moving. `0` is
//! sway's behaviour, for anyone who wants it.
//! `a_window_on_a_dark_monitor_is_told_to_draw_once_a_second_and_not_every_frame`.
//!
//! [sway-frame]: https://github.com/swaywm/sway/blob/1652c54b73f67df17b7b4ab0b0f7048204aa8104/sway/desktop/output.c#L313-L375
//! [niri-fallback]: https://github.com/niri-wm/niri/blob/1f03391ea644c2a43597de7f637269e26d1e1b49/src/niri.rs#L5268-L5336
//! [niri-redraw]: https://github.com/niri-wm/niri/blob/1f03391ea644c2a43597de7f637269e26d1e1b49/src/niri.rs#L4757-L4781
//! [niri-queue]: https://github.com/niri-wm/niri/blob/1f03391ea644c2a43597de7f637269e26d1e1b49/src/backend/tty.rs#L1994-L1998

use std::time::Duration;

use smithay::{
    desktop::Window,
    output::Output,
    reexports::{
        wayland_protocols_wlr::output_power_management::v1::server::{
            zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1},
            zwlr_output_power_v1::{self, ZwlrOutputPowerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            backend::GlobalId,
        },
    },
};

use crate::state::Solium;

/// Registers `zwlr_output_power_manager_v1`.
#[derive(Debug)]
pub(crate) struct PowerState {
    #[expect(dead_code, reason = "holds the global; dropping it would remove it")]
    global: GlobalId,
}

impl PowerState {
    pub(crate) fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrOutputPowerManagerV1, ()> + 'static,
    {
        Self {
            global: display.create_global::<D, ZwlrOutputPowerManagerV1, _>(1, ()),
        }
    }
}

/// Which monitors are off, and who is listening.
#[derive(Debug, Default)]
pub(crate) struct Power {
    /// Monitors asked to be off, by anything.
    off: Vec<Output>,
    /// Of those, the ones the backend has made dark. See "The lock" above.
    dark: Vec<Output>,
    /// Every live `zwlr_output_power_v1`, with the monitor it is about. A
    /// control answered `failed` is never in here, so nothing it asks for
    /// afterwards is acted on
    /// (`a_display_that_will_not_switch_off_is_failed_and_stays_on`).
    controls: Vec<(ZwlrOutputPowerV1, Output)>,
}

impl Power {
    /// Whether `output` has been asked to be off.
    pub(crate) fn is_off(&self, output: &Output) -> bool {
        self.off.contains(output)
    }

    /// Whether the backend has made `output` dark.
    pub(crate) fn is_dark(&self, output: &Output) -> bool {
        self.dark.contains(output)
    }

    /// The mode every control for `output` should hear.
    fn mode(&self, output: &Output) -> zwlr_output_power_v1::Mode {
        if self.is_off(output) {
            zwlr_output_power_v1::Mode::Off
        } else {
            zwlr_output_power_v1::Mode::On
        }
    }

    /// Tell every control for `output` what it is now.
    fn announce(&self, output: &Output) {
        let mode = self.mode(output);
        for (control, of) in &self.controls {
            if of == output && control.is_alive() {
                control.mode(mode);
            }
        }
    }

    /// `failed` to every control for `output`, which stops being one.
    fn fail(&mut self, output: &Output) {
        self.controls.retain(|(control, of)| {
            if of == output {
                control.failed();
                false
            } else {
                true
            }
        });
    }
}

/// What a backend has done to one monitor's display so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Lit {
    /// Showing what it is drawn.
    #[default]
    On,
    /// A frame of black has been queued on it, and it has not been switched
    /// off yet. Only the tty has this: see "What off is, per backend".
    Blanked,
    /// Showing nothing.
    Dark,
}

/// What a backend does with one monitor this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Draw it, as any other frame.
    Draw,
    /// Draw one frame of black on it, instead of its picture.
    Blank,
    /// Switch the display off: the black frame is on it.
    Darken,
    /// Nothing at all: it is dark.
    Rest,
    /// It was dark or going dark and is wanted back: draw it **in full**,
    /// with nothing of any earlier buffer trusted, and it is lit again.
    Wake,
}

/// The whole of what a monitor goes through on its way off and back.
///
/// Off always passes through [`Step::Blank`] before [`Step::Darken`], and a
/// monitor that is wanted back from anywhere on that way is woken with a
/// frame drawn in full. See "What off is, per backend" in the module
/// documentation. `every_way_a_monitor_goes_off_and_on_is_one_step_at_a_time`.
pub(crate) fn step(off: bool, lit: Lit) -> Step {
    match (off, lit) {
        (false, Lit::On) => Step::Draw,
        (false, Lit::Blanked | Lit::Dark) => Step::Wake,
        (true, Lit::On) => Step::Blank,
        (true, Lit::Blanked) => Step::Darken,
        (true, Lit::Dark) => Step::Rest,
    }
}

/// Whether a window is shown only on screens that are off.
///
/// `off` is one entry per monitor the window is on, true for each that is
/// off. A window on no monitor at all -- a workspace parked off every screen
/// -- is dark only when every screen is, because until now it has been told
/// to draw by every monitor's refresh, and one lit screen still does.
/// `a_window_is_in_the_dark_only_if_no_lit_screen_shows_it`, and
/// `a_window_on_a_dark_monitor_is_told_to_draw_once_a_second_and_not_every_frame`.
pub(crate) fn unlit(off: impl IntoIterator<Item = bool>, every_screen_off: bool) -> bool {
    let mut on_any = false;
    for off in off {
        if !off {
            return false;
        }
        on_any = true;
    }
    on_any || every_screen_off
}

impl Solium {
    /// Turn `output` on or off. Whether anything changed.
    ///
    /// The one place a monitor's power changes, whoever asked: see the module
    /// documentation. Told to every control at once, which is what the
    /// protocol means by "effective immediately"; the backend catches up on
    /// the frame this asks for.
    /// `the_power_protocol_is_advertised_answers_mode_on_bind_and_turns_a_monitor_off_and_on`.
    pub(crate) fn set_power(&mut self, output: &Output, on: bool) -> bool {
        if !self.space.outputs().any(|each| each == output) || on != self.power.is_off(output) {
            return false;
        }
        if on {
            self.power.off.retain(|each| each != output);
            // Not dark from this moment, for the lock's sake: a lock still
            // waiting now waits for this monitor to show it.
            // `a_monitor_powered_on_while_locking_must_show_the_lock_too`.
            self.power.dark.retain(|each| each != output);
        } else {
            self.power.off.push(output.clone());
            // A capture waiting for this monitor's next frame is waiting for
            // one that is not coming. `an_off_monitor_cannot_be_captured`.
            let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_captures)
                .into_iter()
                .partition(|capture| capture.output == *output);
            self.pending_captures = kept;
            for capture in gone {
                capture.frame.failed();
            }
        }
        self.power.announce(output);
        self.redraw = true;
        tracing::info!(monitor = output.name(), on, "monitor power");
        true
    }

    /// Every monitor on, or every one off.
    pub(crate) fn power_all(&mut self, on: bool) {
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            self.set_power(&output, on);
        }
    }

    /// Someone is at the machine: every screen that is off comes back on.
    ///
    /// Whoever turned it off (`input_wakes_every_screen_the_idle_blank_turned_off`
    /// plays the idle blank, the rest reach here the same way). And the input
    /// that does it is **delivered as usual**, not swallowed. The case that decides it is a lock screen
    /// that went dark: the first thing typed at it is the password, typed by
    /// someone who has no reason to think the first key does not count. A
    /// compositor that ate it would fail the unlock for a reason nobody can
    /// see, and teach them to press a key and wait before typing, which is a
    /// habit a machine should not have to ask for. niri delivers it the same
    /// way, falling through to its usual handling after `activate_monitors`.
    /// A click on a dark screen lands on whatever is under the pointer, which
    /// is the one argument the other way; the pointer has not moved since the
    /// screen went dark, and on a locked screen the only thing under it is the
    /// lock.
    /// `a_key_that_wakes_a_dark_lock_screen_is_typed_into_the_lock_screen`.
    pub(crate) fn wake_screens(&mut self) {
        if self.power.off.is_empty() {
            return;
        }
        tracing::info!("input: every screen back on");
        self.power_all(true);
    }

    /// What a backend does with `output` this frame, given what it has done
    /// to it so far. See [`step`].
    pub(crate) fn power_step(&self, output: &Output, lit: Lit) -> Step {
        step(self.power.is_off(output), lit)
    }

    /// The backend has made `output` dark.
    ///
    /// Only counted while it is still wanted off: a monitor asked back on in
    /// the meantime is not dark whatever the backend last did to it
    /// (`a_display_that_will_not_switch_off_is_failed_and_stays_on`).
    pub(crate) fn output_dark(&mut self, output: &Output) {
        if !self.power.is_off(output) || self.power.is_dark(output) {
            return;
        }
        self.power.dark.push(output.clone());
        tracing::debug!(monitor = output.name(), "dark");
        // It may be the monitor a lock was waiting for.
        self.confirm_lock();
    }

    /// The backend could not turn `output` off. `failed`, the protocol's own
    /// word for it, and the monitor is on again.
    /// `a_display_that_will_not_switch_off_is_failed_and_stays_on`.
    pub(crate) fn power_refused(&mut self, output: &Output) {
        self.power.fail(output);
        self.power.off.retain(|each| each != output);
        self.power.dark.retain(|each| each != output);
        self.redraw = true;
    }

    /// Forget the monitors that have gone, and tell their controls so.
    ///
    /// Part of `settle_monitors`: "the output disappeared" is one of the
    /// protocol's reasons for `failed`.
    /// `the_power_protocol_is_advertised_answers_mode_on_bind_and_turns_a_monitor_off_and_on`.
    pub(crate) fn settle_power(&mut self) {
        let gone: Vec<Output> = self
            .power
            .controls
            .iter()
            .map(|(_, output)| output)
            .chain(&self.power.off)
            .filter(|output| !self.space.outputs().any(|each| each == *output))
            .cloned()
            .collect();
        for output in gone {
            self.power.fail(&output);
            self.power.off.retain(|each| each != &output);
            self.power.dark.retain(|each| each != &output);
        }
    }

    /// Whether `window` is on screens that are all off. See [`unlit`].
    pub(crate) fn in_the_dark(&self, window: &Window) -> bool {
        if self.power.off.is_empty() {
            return false;
        }
        let every_screen_off = self.space.outputs().next().is_some()
            && self.space.outputs().all(|output| self.power.is_off(output));
        unlit(
            self.space
                .outputs_for_element(window)
                .iter()
                .map(|output| self.power.is_off(output)),
            every_screen_off,
        )
    }

    /// `output` has refreshed: tell its windows they may draw again, at most
    /// once per `throttle`.
    ///
    /// Every window, as it always was, but one shown only on screens that are
    /// off: that one is [`Self::send_dark_frames`]'s. So a lit monitor's
    /// refresh does not keep a window on a dark one drawing at full rate.
    /// `a_window_on_a_dark_monitor_is_told_to_draw_once_a_second_and_not_every_frame`.
    pub(crate) fn send_frames_on(&self, output: &Output, time: Duration, throttle: Duration) {
        for window in self.space.elements() {
            if self.in_the_dark(window) {
                continue;
            }
            window.send_frame(output, time, Some(throttle), |_, _| Some(output.clone()));
        }
    }

    /// Frame callbacks for the windows no lit screen draws, at most one per
    /// `idle.off_frame_interval` each, and none at all for 0. Asked once a
    /// loop iteration by both backends. See "Frame callbacks on a dark
    /// monitor", and
    /// `a_window_on_a_dark_monitor_is_told_to_draw_once_a_second_and_not_every_frame`.
    pub(crate) fn send_dark_frames(&self, time: Duration) {
        let interval = self.idle.settings().off_frame_interval;
        if self.power.off.is_empty() || interval.is_zero() {
            return;
        }
        for window in self.space.elements() {
            if !self.in_the_dark(window) {
                continue;
            }
            let Some(output) = self
                .space
                .outputs_for_element(window)
                .into_iter()
                .next()
                .or_else(|| self.space.outputs().next().cloned())
            else {
                continue;
            };
            // No output is anybody's primary: smithay then sends only to a
            // surface whose last callback is older than `interval`.
            window.send_frame(&output, time, Some(interval), |_, _| None);
        }
    }
}

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrOutputPowerManagerV1,
        request: zwlr_output_power_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let zwlr_output_power_manager_v1::Request::GetOutputPower { id, output } = request else {
            // `destroy`: the controls it made stay valid, as the protocol says.
            return;
        };
        let control = data_init.init(id, ());
        let Some(output) = Output::from_resource(&output)
            .filter(|output| state.space.outputs().any(|each| each == output))
        else {
            // A `wl_output` for a monitor that has gone.
            control.failed();
            return;
        };
        // "Also sent immediately when the object is created."
        control.mode(state.power.mode(&output));
        state.power.controls.push((control, output));
    }
}

impl Dispatch<ZwlrOutputPowerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        control: &ZwlrOutputPowerV1,
        request: zwlr_output_power_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let zwlr_output_power_v1::Request::SetMode { mode } = request else {
            return;
        };
        let on = match mode {
            WEnum::Value(zwlr_output_power_v1::Mode::On) => true,
            WEnum::Value(zwlr_output_power_v1::Mode::Off) => false,
            _ => {
                control.post_error(
                    zwlr_output_power_v1::Error::InvalidMode,
                    "the power modes are on and off",
                );
                return;
            }
        };
        // A control already answered `failed` is not in the list, and asks
        // for nothing any more.
        let Some(output) = state
            .power
            .controls
            .iter()
            .find(|(each, _)| each == control)
            .map(|(_, output)| output.clone())
        else {
            return;
        };
        state.set_power(&output, on);
    }

    fn destroyed(
        state: &mut Self,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        control: &ZwlrOutputPowerV1,
        (): &(),
    ) {
        state.power.controls.retain(|(each, _)| each != control);
    }
}

#[cfg(test)]
mod tests {
    use super::{Lit, Step, step, unlit};

    /// **Every way a monitor goes off and comes back is one step at a
    /// time.** Off is a frame of black and then the display switched off,
    /// never the display switched off with whatever was on it; back on from
    /// anywhere on that way is a frame drawn in full.
    ///
    /// The whole of the tty's sequence, played the way `tty.rs` plays it:
    /// `Blank` leaves the monitor `Blanked`, `Darken` leaves it `Dark`, and
    /// `Wake` leaves it `On`.
    #[test]
    fn every_way_a_monitor_goes_off_and_on_is_one_step_at_a_time() {
        assert_eq!(step(false, Lit::On), Step::Draw);

        assert_eq!(step(true, Lit::On), Step::Blank, "off starts with black");
        assert_eq!(
            step(true, Lit::Blanked),
            Step::Darken,
            "and switches off only once the black is on it"
        );
        assert_eq!(step(true, Lit::Dark), Step::Rest, "and then does nothing");

        assert_eq!(
            step(false, Lit::Dark),
            Step::Wake,
            "back on is drawn in full"
        );
        assert_eq!(
            step(false, Lit::Blanked),
            Step::Wake,
            "and so is back on before it was ever switched off"
        );
    }

    /// Which windows are the dark pass's rather than a lit monitor's.
    #[test]
    fn a_window_is_in_the_dark_only_if_no_lit_screen_shows_it() {
        assert!(unlit([true], false), "on one screen, which is off");
        assert!(unlit([true, true], false), "on two, both off");
        assert!(!unlit([true, false], false), "straddling a lit one");
        assert!(!unlit([false], false), "on a lit one");
        assert!(
            !unlit(std::iter::empty(), false),
            "on none, with a lit screen still refreshing"
        );
        assert!(
            unlit(std::iter::empty(), true),
            "on none, with every screen off"
        );
    }
}
