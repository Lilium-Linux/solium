//! Rounds: the windows a layout's edge drag moves, moved together
//! (`resize.sync`).
//!
//! A tiled seam is two windows and two applications, and each window is drawn
//! with the picture its own application last committed. Moved the moment the
//! pointer moves -- which is what the bridge did on its own -- the two are only
//! ever right together when both applications happen to have caught up. The
//! rest of the time one tile's picture stops short of the seam and
//! `resize.background` shows in the gap, and the other's is wider than its
//! tile and is cut by the seam: on 2026-10-16 that was a grey band down the
//! middle of every fast seam drag, and the content of the faster side cut off.
//!
//! So the panes a drag moves move in rounds:
//!
//! 1. **Told together.** Every pane whose slot has moved past what its client
//!    was last told is told now, in one go, and remembers the configure it
//!    was sent (`Solium::start_round`). Between rounds the layout goes on
//!    moving the slots with the pointer, and nobody is told.
//! 2. **Answers held back.** A commit that acknowledges that configure is held
//!    by a commit blocker (`Solium::round_commit`), so the screen keeps the
//!    picture that matches where the pane is still drawn: where the last round
//!    put it, which is [`Solium::seam_shown_of`].
//! 3. **Let go together.** Once every pane has answered, or `resize.sync` has
//!    passed since they were told, every held answer is applied at once and
//!    every pane is drawn at the rectangle its round asked for
//!    (`Solium::end_round`): the one frame that shows the new seam shows each
//!    window drawn for it. The next round starts on the same pass, from
//!    wherever the pointer has got to.
//!
//! A client that answers with a size of its own still answered, and is shown
//! at its round's rectangle with the bridge's fill covering the difference, as
//! before; so is one that has not answered by the deadline. A pane that only
//! moves has no configure to answer -- a Wayland client is not told where it
//! is -- and is ready at once; so is an X11 window, whose configure has no
//! acknowledgement to wait for.
//!
//! **Only the bridge's panes**, the ones a layout's drag moved. A floating drag
//! follows its own client (`resize.follow`) and has nothing to wait for.

use std::time::Duration;

use smithay::{
    desktop::Window,
    reexports::wayland_server::{Client, Resource as _, protocol::wl_surface::WlSurface},
    utils::{Logical, Rectangle, Serial},
    wayland::compositor::{Barrier, CompositorHandler as _, add_blocker, with_states},
};

use super::{Pane, Solium};

/// The sizes a layout's drag told its panes together, and which of their
/// clients have answered.
#[derive(Debug)]
pub(crate) struct Round {
    /// When they were told. `resize.sync` runs from here.
    began: Duration,
    /// What holds every answer back until the round ends: one for the round,
    /// added to each answering commit as its blocker, and signalled once.
    barrier: Barrier,
    members: Vec<Member>,
}

/// One pane of a [`Round`].
#[derive(Debug)]
struct Member {
    pane: crate::pane::PaneId,
    window: Window,
    /// The client rectangle it was told: where it is drawn once the round
    /// ends.
    asked: Rectangle<i32, Logical>,
    /// The configure that told it, for a Wayland client that was sent one.
    /// `None` -- nothing to wait for -- for a pane that only moved and for an
    /// X11 window.
    serial: Option<Serial>,
    /// Whether its answer is in: held back by the round's barrier, or never
    /// owed.
    answered: bool,
}

/// The last configure a toplevel acknowledged, kept on its surface by
/// `XdgShellHandler::ack_configure` for [`Solium::round_commit`] to read: the
/// acknowledgement comes before the commit it belongs to, and Smithay keeps
/// which configure was acknowledged but not its serial.
#[derive(Debug, Default)]
pub(crate) struct Acked(std::sync::Mutex<Option<Serial>>);

/// Remember that `surface` acknowledged the configure `serial`.
pub(crate) fn note_acked(surface: &WlSurface, serial: Serial) {
    with_states(surface, |states| {
        states.data_map.insert_if_missing_threadsafe(Acked::default);
        if let Some(acked) = states.data_map.get::<Acked>()
            && let Ok(mut held) = acked.0.lock()
        {
            *held = Some(serial);
        }
    });
}

/// The last configure `surface` acknowledged, if any.
fn acked(surface: &WlSurface) -> Option<Serial> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<Acked>()
            .and_then(|acked| acked.0.lock().ok().and_then(|held| *held))
    })
}

impl Solium {
    /// Whether rounds pace the bridge's panes: `resize.sync` is not zero.
    pub(super) fn syncing(&self) -> bool {
        !self.resizing.sync.is_zero()
    }

    /// Where the bridge draws `pane` while rounds pace it, if they do.
    pub(crate) fn seam_shown_of(&self, pane: &Pane) -> Option<Rectangle<i32, Logical>> {
        if !self.syncing() {
            return None;
        }
        self.seam_shown
            .iter()
            .find(|(each, _)| *each == pane.id())
            .map(|&(_, rect)| rect)
    }

    /// Whether `pane` is in the round under way.
    pub(super) fn in_round(&self, pane: crate::pane::PaneId) -> bool {
        self.seam_round
            .as_ref()
            .is_some_and(|round| round.members.iter().any(|member| member.pane == pane))
    }

    /// A pane joins the bridge drawn where it is now: `rect`, read before the
    /// layout's move. Kept until a round moves it.
    pub(super) fn show_from(&mut self, pane: crate::pane::PaneId, rect: Rectangle<i32, Logical>) {
        if self.seam_shown.iter().all(|(each, _)| *each != pane) {
            self.seam_shown.push((pane, rect));
        }
    }

    /// The pre-commit hook on every toplevel: a commit answering this
    /// window's configure in the round under way is held back until the round
    /// ends. See the module's step 2.
    pub(crate) fn round_commit(&mut self, surface: &WlSurface) {
        let Some(round) = self.seam_round.as_mut() else {
            return;
        };
        let Some(member) = round.members.iter_mut().find(|member| {
            !member.answered
                && member
                    .window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == surface)
        }) else {
            return;
        };
        let (Some(asked), Some(acked)) = (member.serial, acked(surface)) else {
            return;
        };
        if acked.is_no_older_than(&asked) {
            add_blocker(surface, round.barrier.clone());
            member.answered = true;
        }
    }

    /// Once a frame, before the bridge settles: end the round under way when
    /// every pane has answered or `resize.sync` has run out, and start the
    /// next for whatever the layout has moved since. See the module.
    pub(super) fn settle_round(&mut self, now: Duration) {
        let bridged: Vec<(Window, crate::pane::PaneId)> = self
            .resize_bridge
            .iter()
            .flat_map(|bridge| &bridge.panes)
            .map(|held| (held.window.clone(), held.pane))
            .collect();
        // Nothing is drawn from a rectangle for a pane the bridge has let go.
        self.seam_shown
            .retain(|(pane, _)| bridged.iter().any(|(_, each)| each == pane));
        if let Some(round) = self.seam_round.as_ref() {
            // A pane that has left the bridge, or whose client has, is waited
            // on no longer: whatever it was going to say no longer decides
            // where anything is drawn.
            let waiting = round.members.iter().any(|member| {
                !member.answered
                    && bridged
                        .iter()
                        .any(|(window, pane)| *pane == member.pane && *window == member.window)
            });
            let late = now.saturating_sub(round.began) >= self.resizing.sync;
            if waiting && !late && self.syncing() {
                return;
            }
            if let Some(round) = self.seam_round.take() {
                self.end_round(round);
            }
        }
        if self.syncing() {
            self.start_round(now);
        }
    }

    /// Tell every bridged pane whose slot has moved past what its client was
    /// last told, all at once, as one round. See the module's step 1.
    fn start_round(&mut self, now: Duration) {
        let watching: Vec<(Window, crate::pane::PaneId)> = self
            .resize_bridge
            .iter()
            .flat_map(|bridge| &bridge.panes)
            .map(|held| (held.window.clone(), held.pane))
            .collect();
        let mut members = Vec::new();
        for (window, pane) in watching {
            // The pane has gone, or has been given another client: the same
            // check every pass over the bridge makes.
            if self.panes.get(pane).and_then(Pane::client) != Some(&window) {
                continue;
            }
            let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
                continue;
            };
            let committed = window.geometry().size;
            let Some(held) = self
                .resize_bridge
                .as_mut()
                .and_then(|bridge| bridge.panes.iter_mut().find(|held| held.pane == pane))
            else {
                continue;
            };
            // Unthrottled: the round is the pace, and `resize.tell_hz` has no
            // business making one pane of it wait for another interval.
            let Some(tell) = held.hold.placed(slot, committed, now) else {
                continue;
            };
            let serial = super::size_window(&window, tell);
            members.push(Member {
                pane,
                window,
                asked: tell,
                serial,
                answered: serial.is_none(),
            });
        }
        if members.is_empty() {
            return;
        }
        self.redraw = true;
        if crate::resizing::trace::on() {
            for member in &members {
                crate::resizing::trace::line(
                    "round",
                    format_args!(
                        "pane={} asked={},{} {}x{} waits={}",
                        member.pane.get(),
                        member.asked.loc.x,
                        member.asked.loc.y,
                        member.asked.size.w,
                        member.asked.size.h,
                        u8::from(!member.answered),
                    ),
                );
            }
        }
        let round = Round {
            began: now,
            barrier: Barrier::new(false),
            members,
        };
        // Only moves, which nobody answers: they land together on this pass.
        if round.members.iter().all(|member| member.answered) {
            self.end_round(round);
            return;
        }
        self.seam_round = Some(round);
    }

    /// Let every held answer go and draw every pane where its round put it.
    /// See the module's step 3.
    ///
    /// **The one way a round ends**, so a held answer is never left behind: a
    /// commit held by a barrier nobody signals is a client waiting for ever on
    /// the frame callback that commit carries.
    fn end_round(&mut self, round: Round) {
        round.barrier.signal();
        for member in &round.members {
            match self
                .seam_shown
                .iter_mut()
                .find(|(pane, _)| *pane == member.pane)
            {
                Some((_, shown)) => *shown = member.asked,
                None => self.seam_shown.push((member.pane, member.asked)),
            }
        }
        // Each client once: clearing a blocker applies every commit of that
        // client's that is ready, whichever of its windows it was for.
        let mut clients: Vec<Client> = Vec::new();
        for member in &round.members {
            let Some(client) = member
                .window
                .toplevel()
                .and_then(|toplevel| toplevel.wl_surface().client())
            else {
                continue;
            };
            if !clients.contains(&client) {
                clients.push(client);
            }
        }
        let display = self.display_handle.clone();
        for client in clients {
            self.client_compositor_state(&client)
                .blocker_cleared(self, &display);
        }
        if crate::resizing::trace::on() {
            crate::resizing::trace::line(
                "round",
                format_args!(
                    "ended answered={}/{}",
                    round
                        .members
                        .iter()
                        .filter(|member| member.answered)
                        .count(),
                    round.members.len(),
                ),
            );
        }
        self.redraw = true;
    }
}
