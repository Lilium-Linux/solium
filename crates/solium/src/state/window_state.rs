//! Fullscreen and maximised, for a window of either kind (#361): the two
//! states as the compositor last set them, read and written where each kind
//! keeps them, and the way into fullscreen and out of it that both kinds take.
//!
//! An xdg toplevel keeps its states in the configure it is about to be sent.
//! An X11 window keeps them in its `_NET_WM_STATE`, which
//! `X11Surface::set_fullscreen` and `set_maximized` write and nothing else
//! does: smithay never reads the client's own (`xwayland.rs`,
//! `floats_over_its_parent`, says why), so what is there is what we set. How
//! each kind is told its rectangle differs too: an xdg client is sent a size,
//! and picks its own when sent none; an X11 window is told its whole
//! rectangle, position included, and keeps the one it has when told nothing.
//!
//! Everything else is the pane's and is written once, here and in
//! `Solium::toggle_maximize`: the way back, the tile left and gone back to,
//! the bare frame and its return, the stacking over the bars that
//! `fullscreen.covers` asks for, the monitor, and the motion. Until #361 all of
//! it started from `Window::toplevel`, so an X11 window -- a Proton game asking
//! for fullscreen -- was neither fullscreen nor maximised, and super+f and
//! super+shift+m did nothing to it.

use super::*;

/// Whether a window is fullscreen and maximised, as the compositor last set
/// it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct States {
    pub(crate) fullscreen: bool,
    pub(crate) maximized: bool,
}

/// How far, in logical pixels, a maximised window's drag goes before the
/// window comes off: `maximize.restore_after`'s default (#452). A few pixels,
/// so a click and a double click on its titlebar, which a hand moves by a
/// pixel or two, stay clicks.
pub(crate) const RESTORE_AFTER: f64 = 6.0;

/// The most `maximize.restore_after` may be: past this a maximised window is
/// as good as fixed in place.
pub(crate) const RESTORE_AFTER_MAX: f64 = 200.0;

/// What `config.lua`'s `maximize` section says to the compositor
/// (`sol.maximize`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Maximize {
    /// How far a drag of a maximised window goes, from where it was taken,
    /// before the window is restored under the pointer (#452). Until then it
    /// does not move.
    /// `crate::input::grab::MoveDrag::to`.
    pub(crate) restore_after: f64,
}

impl Default for Maximize {
    fn default() -> Self {
        Self {
            restore_after: RESTORE_AFTER,
        }
    }
}

/// A window's [`States`], whichever kind it is: for an xdg toplevel the state
/// its next configure carries, so nothing waits for the client, and for an X11
/// window its `_NET_WM_STATE`. The one answer for everything that asks: the
/// stacking over the bars, a rule's `fullscreen`, `sol.windows()`, the windows
/// model, and the toggles themselves.
pub(crate) fn states(window: &Window) -> States {
    if let Some(toplevel) = window.toplevel() {
        return toplevel.with_pending_state(|state| States {
            fullscreen: state.states.contains(xdg_toplevel::State::Fullscreen),
            maximized: state.states.contains(xdg_toplevel::State::Maximized),
        });
    }
    window
        .x11_surface()
        .map_or_else(States::default, |x11| States {
            fullscreen: x11.is_fullscreen(),
            maximized: x11.is_maximized(),
        })
}

/// Set or clear fullscreen where the window's kind keeps it. An xdg client
/// hears it with the next configure, which [`tell`] sends; an X11 one at once,
/// as a property.
pub(super) fn write_fullscreen(window: &Window, fullscreen: bool) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            if fullscreen {
                state.states.set(xdg_toplevel::State::Fullscreen);
            } else {
                state.states.unset(xdg_toplevel::State::Fullscreen);
            }
        });
    } else if let Some(x11) = window.x11_surface()
        && let Err(err) = x11.set_fullscreen(fullscreen)
    {
        tracing::warn!(
            ?err,
            "could not tell an X11 window whether it is fullscreen"
        );
    }
}

/// And maximised, the same way.
pub(super) fn write_maximized(window: &Window, maximized: bool) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            if maximized {
                state.states.set(xdg_toplevel::State::Maximized);
            } else {
                state.states.unset(xdg_toplevel::State::Maximized);
            }
        });
    } else if let Some(x11) = window.x11_surface()
        && let Err(err) = x11.set_maximized(maximized)
    {
        tracing::warn!(?err, "could not tell an X11 window whether it is maximised");
    }
}

/// When an xdg client is sent what [`tell`] wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sending {
    /// Only once it has had its initial configure: the fullscreen requests,
    /// which a client may make before its first commit, and whose state then
    /// goes out with that initial configure.
    OnceConfigured,
    /// At once, as `toggle_maximize` has always sent it.
    Now,
}

/// Tell a window the rectangle it now has, and whether it is tiled, with the
/// states written beside them -- in one configure for an xdg client, so it
/// never resizes once for the size and again for the states.
///
/// `None` is no rectangle: an xdg client is sent no size and picks its own,
/// and an X11 window, which has no such answer, keeps the one it has. `tiled`
/// is xdg's alone (#228); `None` leaves it as it is. An override-redirect X11
/// window is never told anything, for `size_window`'s reason.
pub(super) fn tell(
    window: &Window,
    rect: Option<Rectangle<i32, Logical>>,
    tiled: Option<bool>,
    send: Sending,
) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            state.size = rect.map(|rect| rect.size);
            if let Some(tiled) = tiled {
                placement::set_tiled(state, tiled);
            }
        });
        if send == Sending::Now || toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        return;
    }
    if let Some(x11) = window.x11_surface()
        && !x11.is_override_redirect()
        && let Some(rect) = rect
        && let Err(err) = x11.configure(Some(rect))
    {
        tracing::warn!(?err, "could not size an X11 window");
    }
}

/// Answer a client's request to be maximised, or not, that changes nothing:
/// with the state and the size it has, as xdg has the compositor answer
/// every such request with a configure. An X11 client is sent its own
/// rectangle again, which is the answer it waits for.
fn answer_unchanged(window: &Window) {
    if let Some(toplevel) = window.toplevel() {
        if toplevel.is_initial_configure_sent() {
            toplevel.send_configure();
        }
    } else if let Some(x11) = window.x11_surface()
        && !x11.is_override_redirect()
        && let Err(err) = x11.configure(None)
    {
        tracing::warn!(?err, "could not answer an X11 window's maximise");
    }
}

/// Whether a window's client draws its own frame, so leaving fullscreen
/// builds none over it.
///
/// An xdg client says so through `xdg_decoration`, and is asked again rather
/// than assumed, since `is_bare` cannot tell a frame that was dropped for the
/// fullscreen from one that was never wanted. An X11 window never does: every
/// managed one is given a frame at its first frame (`show_if_new`, "a client
/// that never negotiates still gets a frame"), so leaving fullscreen gives it
/// one back.
fn draws_its_own_frame(window: &Window) -> bool {
    window.toplevel().is_some_and(|toplevel| {
        toplevel.with_pending_state(|state| state.decoration_mode) == Some(Mode::ClientSide)
    })
}

/// What a client's own request to be maximised, or not, does to its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaximizeAnswer {
    /// It is already as it asks.
    Nothing,
    /// Maximised or restored, as the frame's button and super+shift+m do it.
    Toggle,
    /// Fullscreen, so only the state under the fullscreen one changes, which
    /// is where leaving fullscreen goes: maximised, or back to the rect kept
    /// from before either.
    Underneath,
}

/// Who asked for a maximise, or for one to end: what decides whether a
/// layout's tile outranks it (`Pane::maximized_by_client`, #450).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asker {
    /// The person at the machine: super+shift+m, `sol.toggle_maximize`, the
    /// frame's button or a double click on its titlebar. Lifts a window out
    /// of its tile (#150).
    User,
    /// The window's own client: xdg `set_maximized` and `unset_maximized`,
    /// or an X11 `_NET_WM_STATE` request.
    Client,
}

/// How a window answers its client asking to be maximised (`wants`) or not.
///
/// **Underneath while fullscreen**, rather than `Solium::toggle_maximize`'s
/// answer to a key. A person pressing maximise on a fullscreen window wants to
/// see it maximised, so the key leaves fullscreen for it. A client writing
/// `_NET_WM_STATE_MAXIMIZED` while fullscreen is saying what it should be once
/// it leaves -- EWMH's states are independent, and fullscreen wins while it
/// holds -- and leaving fullscreen on that would take a game out of
/// fullscreen because it tidied its own state. Both halves keep the one way
/// back on the pane right: a window fullscreen from maximised keeps the rect
/// from before the maximise, and one fullscreen from neither keeps its own,
/// which is the rect an un-maximise after leaving takes either way.
/// `window_state::tests::a_client_maximising_while_fullscreen_changes_only_the_state_underneath`.
pub(crate) const fn answer_maximize(states: States, wants: bool) -> MaximizeAnswer {
    if states.maximized == wants {
        MaximizeAnswer::Nothing
    } else if states.fullscreen {
        MaximizeAnswer::Underneath
    } else {
        MaximizeAnswer::Toggle
    }
}

impl Solium {
    /// A window asking for the whole screen, by its client or by a key.
    ///
    /// Not the same as maximised, and the difference is the whole point: a
    /// maximised window fills the *work area* and keeps its frame, a
    /// fullscreen one covers the monitor edge to edge with no frame and no
    /// bar over it. A video player, a game, a presentation. Without this the
    /// request was ignored entirely — the client had asked, been told nothing,
    /// and drew its own idea of fullscreen inside a titlebar.
    ///
    /// `output` is the monitor an xdg client named. With none, the monitor the
    /// window is on, not the active one: a video sent fullscreen on the second
    /// screen must not jump to whichever screen the pointer is over.
    ///
    /// A window that places itself -- an X11 menu, tooltip or
    /// override-redirect window -- is not made fullscreen: nothing about it is
    /// the compositor's to size.
    pub(crate) fn fullscreen(&mut self, window: &Window, output: Option<Output>) {
        let Some(id) = self.panes.id_of(window) else {
            return;
        };
        if !self.panes.get(id).is_some_and(Pane::managed) {
            return;
        }
        let output = output
            .or_else(|| {
                self.real_geometry(window)
                    .and_then(|real| self.output_of(real))
            })
            .or_else(|| self.active_output());
        let Some(screen) = output.and_then(|output| self.space.output_geometry(&output)) else {
            return;
        };
        // A drag holding it lets go: the monitor is where it goes, and a drag
        // that went on moving it left it fullscreen at an offset (#404).
        crate::input::grab::let_go_of(self, window);

        // Where to come back to, kept before anything moves. The same slot a
        // maximise keeps, and for the same reason: a rect that was stored is a
        // rect that comes back exactly, where one recomputed afterwards is a
        // guess.
        //
        // Not by a window that is fullscreen already: a client may ask a
        // second time, and the rect it has by then is the monitor's. Asked of
        // the window's state rather than of whether a rect is kept, because a
        // window that went fullscreen before it had drawn has none kept (see
        // below), and its second request would otherwise keep the monitor.
        //
        // Not over a rect kept already, either. A window maximised and then
        // sent fullscreen keeps the rect from before the maximise, and it stays
        // there while `unfullscreen` puts the window back to maximised, for the
        // un-maximise after that to take.
        //
        // And not a rect of no size. `new_toplevel` maps a window at 0,0 before
        // it has a buffer, so one asking for fullscreen before its first
        // commit -- a player started with `--fs` -- is there with no size, and
        // a rect that describes nothing is no way back. With none kept,
        // leaving fullscreen lets the client pick its own size.
        let already = states(window).fullscreen;
        // Where it is drawn now, which is where it grows from: read before
        // anything below moves it or drops its frame.
        let start = self.drawn_before(id);
        // The way back is where it stands, and not where the space has it:
        // turned round part of the way out of fullscreen, the space has the
        // monitor's size, which its client has not left yet.
        // `turning_round_part_of_the_way_out_keeps_the_way_back`.
        if !already
            && let Some(real) = self.standing(window)
            && !real.is_empty()
            && let Some(pane) = self.panes.get_mut(id)
            && pane.restore().is_none()
        {
            pane.set_restore(Some(real));
        }
        // Out of its tile, for `toggle_maximize`'s reason: a fullscreen window
        // cut down to the tile it came from is a video playing in a corner of
        // the monitor. Unconditionally rather than beside the rect above,
        // because a window that is already fullscreen, or was maximised first,
        // has left its tile already -- and `leave_tile` keeps that older way
        // back when there is no tile to take.
        if let Some(pane) = self.panes.get_mut(id) {
            pane.leave_tile();
        }

        // The whole monitor, and no frame over it. The frame is dropped and
        // the pane marked bare, and leaving fullscreen builds a new one from
        // the style that is current then.
        //
        // Which is why the way back is kept on the pane and not on the frame:
        // it was kept on the frame until #92, and `remove` below dropped it
        // with the frame, so leaving fullscreen never had a rect to put any
        // window back at.
        self.decorations.remove(&mut self.panes, id);
        self.decorations.set_bare(&mut self.panes, id);

        // Out of its tile, so not tiled either (#228).
        write_fullscreen(window, true);
        tell(window, Some(screen), Some(false), Sending::OnceConfigured);
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_slot(screen);
        }
        self.map_stacked(window.clone(), screen.loc, true);
        // And grown there on screen, with the motion the scripts answer
        // `fullscreen` with. Not for a second request from a window that is
        // fullscreen already: nothing has changed to tell.
        // `a_window_glides_into_fullscreen_and_out_again`.
        if !already {
            self.transition(id, Change::Fullscreen, true, start);
        }
        self.redraw = true;
        tracing::debug!(?screen, "a window went fullscreen");
    }

    /// And leaving it.
    ///
    /// Back to maximised if the window was maximised when it went fullscreen,
    /// and otherwise to the rect it had before. Maximise and fullscreen keep
    /// their way back in the one slot on the pane, and that is what the two
    /// checks below are about.
    pub(crate) fn unfullscreen(&mut self, window: &Window) {
        let Some(id) = self.panes.id_of(window) else {
            return;
        };

        // Only a window that is fullscreen has anything to leave. A client may
        // send this whenever it likes, and one that sent it while merely
        // maximised had the maximise's way back spent on it: the window jumped
        // to its pre-maximise rect still marked maximised, and the next toggle
        // maximised it again rather than restoring it.
        let States {
            fullscreen,
            maximized,
        } = states(window);
        if !fullscreen {
            return;
        }
        // Where it is drawn now, which is where it shrinks from: read before
        // its frame comes back and it is moved.
        let start = self.drawn_before(id);

        // The frame comes back unless the client draws its own.
        if !draws_its_own_frame(window) {
            self.decorations.unset_bare(&mut self.panes, id);
            let size = self
                .real_geometry(window)
                .map_or((TITLEBAR_HEIGHT * 20, TITLEBAR_HEIGHT * 15), |real| {
                    (real.size.w, real.size.h)
                });
            self.decorations.insert(&mut self.panes, id, size.0, size.1);
        }

        // Where it goes, decided after the frame is back -- a maximised
        // window's share of the work area depends on it -- and before the
        // client is told anything, so that it is told once. It used to be two
        // configures, no size and then the size to go back to, and a client
        // that acts on every configure it reads, a terminal reflowing its grid,
        // resized twice.
        let back = if maximized {
            // Still maximised: nothing has un-maximised it. So it fills the
            // work area of the monitor it is on now, and the rect from before
            // the maximise stays kept for the un-maximise to take. Taking it
            // here placed and sized the window un-maximised while its state
            // still said `Maximized`, and left the next toggle nothing to
            // restore.
            self.real_geometry(window)
                .and_then(|real| self.maximised(window, real))
        } else {
            // Back into the tile it left as well, with the rect that goes with
            // it; see `toggle_maximize`. A window still maximised stays out of
            // one, which is the arm above.
            let kept = self.panes.get_mut(id).and_then(|pane| {
                pane.return_to_tile();
                pane.take_restore()
            });
            kept.map(|kept| self.back_on_a_screen(window, kept))
        };
        // Back in its tile or not, and told so with the size (#228).
        let tiled = self
            .panes
            .get(id)
            .is_some_and(|pane| pane.placed().is_some());

        // No rect, no size: an xdg client picks its own.
        write_fullscreen(window, false);
        tell(window, back, Some(tiled), Sending::OnceConfigured);
        if let Some(back) = back {
            if let Some(pane) = self.panes.get_mut(id) {
                pane.set_slot(back);
            }
            self.map_stacked(window.clone(), back.loc, true);
        }
        // Back in a tile, and the layout says where that is now, as
        // `toggle_maximize` asks it. Only then: a window going back to where
        // it floated is there already, and a sweep now would place it again
        // from the size its client has not changed yet -- `dialogs.lua`
        // centres a floated window at the size it has, which is still the
        // monitor's.
        // `a_floated_window_made_fullscreen_is_left_so_by_a_sweep_and_goes_back_where_it_floated`.
        if tiled {
            self.trigger_relayout();
        }
        // And shrunk there on screen, from the monitor it covered, with the
        // motion the scripts answer `fullscreen` with -- into the tile the
        // layout has just given it, too. Not with no rectangle to go to, which
        // its client picks.
        // `a_window_glides_into_fullscreen_and_out_again`, and
        // `tests/scenarios/fullscreen-tiled.lua` for the tile.
        self.transition(
            id,
            Change::Fullscreen,
            false,
            start.filter(|_| back.is_some() || tiled),
        );
        self.redraw = true;
        tracing::debug!("a window left fullscreen");
    }

    /// An X11 window that mapped covering `screen`, made fullscreen there
    /// before it has drawn: `xwayland::maps_fullscreen` says why that is a
    /// request.
    ///
    /// **With a way back of its own**, which [`Self::fullscreen`] would not
    /// keep. The rect such a window stands at is the monitor, and kept, leaving
    /// fullscreen put it back over the monitor with its frame above the top
    /// edge. Kept here instead is the work area of that monitor less the frame
    /// it will have by then, which is what `show_if_new` fits a window of the
    /// monitor's size into when it opens framed: so a game switched to
    /// windowed, or a key pressed, lands it as it would have opened had it not
    /// asked. Into the tile the layout gave it, rather, where the layout has
    /// one: `unfullscreen` goes back into a tile first.
    pub(crate) fn fullscreen_from_the_start(
        &mut self,
        window: &Window,
        screen: Rectangle<i32, Logical>,
    ) {
        let Some(id) = self.panes.id_of(window) else {
            return;
        };
        // `insets_of`, not `frame_insets`: the frame is not built yet, and it
        // is the room a frame on its way reserves that the way back makes.
        let back = self
            .work_area_of(screen)
            .map(|area| inner(area, self.insets_of(id)));
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_restore(back);
        }
        let output = self.output_of(screen);
        self.fullscreen(window, output);
    }

    /// Fullscreen if it is not, and back if it is: `sol.toggle_fullscreen`,
    /// super+f, and a hosted scene's `windows.fullscreen`, made as the
    /// window's own client would ask.
    /// `a_script_toggles_fullscreen_and_maximised_as_the_client_would`.
    pub(crate) fn toggle_fullscreen(&mut self, window: &Window) {
        if states(window).fullscreen {
            self.unfullscreen(window);
        } else {
            self.fullscreen(window, None);
        }
    }

    /// A client asking to be maximised, or not: [`answer_maximize`]'s answer,
    /// carried out -- the same for an xdg client (#445) and an X11 one
    /// (#361).
    ///
    /// **Not out of a layout's tile** (#450). A client maximising itself is
    /// an application restoring the state it had last time -- Steam, every
    /// Electron application, a browser -- or its own button, and a window
    /// maximised over its tile sat on top of it, its neighbour laid out
    /// beside an empty tile, and no seam could size it. So a window a layout
    /// holds in a tile, or keeps one for while it is fullscreen, stays as the
    /// layout has it, and is told so: it is tiled and not maximised. One that
    /// has no tile yet -- asking before its first frame, which is when an
    /// application restores its state -- is maximised, and the layout's tile
    /// takes it out of the maximise when it is given (`Solium::move_pane`).
    /// The user's maximise, by key or the frame's button, still leaves the
    /// tile and keeps it to come back to (#150); floating, nothing holds the
    /// window and the client's maximise is carried out.
    /// `real_client::client_maximize`.
    pub(crate) fn client_maximize(&mut self, window: &Window, wants: bool) {
        let Some(pane) = self.panes.of(window).filter(|pane| pane.managed()) else {
            answer_unchanged(window);
            return;
        };
        let tiled = pane.placed().is_some() || pane.left_tile().is_some();
        match answer_maximize(states(window), wants) {
            MaximizeAnswer::Toggle if wants && tiled => answer_unchanged(window),
            MaximizeAnswer::Toggle => self.maximize_toggled(window, Asker::Client),
            MaximizeAnswer::Underneath if wants && tiled => answer_unchanged(window),
            MaximizeAnswer::Underneath => {
                write_maximized(window, wants);
                if let Some(pane) = self
                    .panes
                    .id_of(window)
                    .and_then(|id| self.panes.get_mut(id))
                {
                    pane.set_maximized_by_client(wants);
                }
                answer_unchanged(window);
            }
            MaximizeAnswer::Nothing => answer_unchanged(window),
        }
    }

    /// A maximised window whose frame has just been built, or dropped, while
    /// it was maximised: fitted to the work area again, frame included, so
    /// its titlebar is inside it and nothing of the work area is left over.
    ///
    /// A window maximised before its first frame was given the work area less
    /// the room a frame on its way reserves (`Solium::maximised`), and the
    /// frame that is built may take another amount: a style whose titlebar
    /// is grown (`growTop`, #423) reserves a band and grows into the rest, and
    /// a client that draws its own frame takes none.
    pub(super) fn refit_maximised(&mut self, window: &Window) {
        let States {
            fullscreen,
            maximized,
        } = states(window);
        if fullscreen || !maximized {
            return;
        }
        let Some(id) = self.panes.id_of(window) else {
            return;
        };
        let Some(slot) = self.panes.get(id).map(Pane::slot) else {
            return;
        };
        let Some(filled) = self.maximised(window, slot) else {
            return;
        };
        if filled == slot {
            return;
        }
        tell(window, Some(filled), None, Sending::OnceConfigured);
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_slot(filled);
        }
        self.map_stacked(window.clone(), filled.loc, false);
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::{MaximizeAnswer, States, answer_maximize};

    /// **#361: a client maximising while fullscreen changes only the state
    /// underneath.** A Wine game tidying its own `_NET_WM_STATE` while it is
    /// fullscreen must stay fullscreen; leaving fullscreen then goes where
    /// that state says. Out of fullscreen a request is the toggle a key makes,
    /// and a request for what the window already is does nothing.
    #[test]
    fn a_client_maximising_while_fullscreen_changes_only_the_state_underneath() {
        let states = |fullscreen, maximized| States {
            fullscreen,
            maximized,
        };
        for fullscreen in [false, true] {
            for maximized in [false, true] {
                assert_eq!(
                    answer_maximize(states(fullscreen, maximized), maximized),
                    MaximizeAnswer::Nothing,
                    "fullscreen {fullscreen}: asking to be what it is changed something"
                );
            }
        }
        for wants in [false, true] {
            assert_eq!(
                answer_maximize(states(false, !wants), wants),
                MaximizeAnswer::Toggle,
                "a window out of fullscreen asking {wants} is maximised or restored"
            );
            assert_eq!(
                answer_maximize(states(true, !wants), wants),
                MaximizeAnswer::Underneath,
                "a fullscreen window asking {wants} left fullscreen for it"
            );
        }
    }
}
