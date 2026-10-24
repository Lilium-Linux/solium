//! The pointer taken somewhere by a script (#220): `sol.warp`, and the warp a
//! `sol.focus` can carry. The policy -- which focus changes take the pointer
//! along, where on the window and how -- is Lua's (`lua/follow.lua`); this is
//! the primitive.
//!
//! **The pointer goes at once; its picture flies.** What the pointer is over,
//! what is hovered, which client has it and the shape it shows are the
//! destination's from the moment the warp is carried out, so a button pressed
//! or a key typed the instant after goes where the pointer went. Only the
//! picture travels, from wherever it was drawn, along the motion the script
//! asked for: the split panes make, input at the real place and the drawing
//! presented. [`crate::cursor::flight`] is the picture's half.
//! `tests::real_client::follow::a_key_takes_the_pointer_to_the_window_it_focuses_at_once_and_flies_its_picture`.
//!
//! **The hand on the mouse wins.** Real motion mid-flight ends the flight at
//! once and carries on from where the warp put the pointer -- or, for a warp
//! whose `interrupt` says so, from where the picture is drawn -- and a press
//! lands it, so the click and the picture agree. The jump a warp makes is no
//! motion of the mouse's, so it is never measured as the pointer's speed. A
//! warp asked for while a press is being handled -- a click whose focus takes
//! the pointer along -- waits for the buttons to come up, so the click goes
//! where it was aimed. A warp is refused while a button holds the pointer (a
//! drag, a menu, a scene's press or grab), while a client holds it (a locked
//! or confined pointer), and while the session is locked.
//! `tests::real_client::follow::real_motion_mid_flight_wins_at_once`,
//! `tests::real_client::follow::with_focus_following_the_mouse_a_nudge_mid_flight_keeps_the_keyboard_where_the_key_put_it`,
//! `tests::real_client::follow::real_motion_after_a_warp_reports_only_its_own_speed`,
//! `tests::real_client::follow::a_press_mid_flight_lands_it`,
//! `tests::real_client::follow::a_click_that_takes_the_pointer_along_lands_where_it_was_aimed`,
//! `tests::real_client::follow::a_click_that_opened_a_menu_takes_the_pointer_nowhere_later`,
//! `tests::real_client::follow::a_held_pointer_is_not_warped`,
//! `tests::real_client::lock_focus::no_warp_moves_the_pointer_behind_the_lock`.
//!
//! **It moves no focus.** Focus following the mouse is the mouse's: a warp is
//! the keyboard's doing, and a window over the point it lands on must not take
//! the keyboard off the one that was focused.
//! `tests::real_client::follow::a_warp_takes_no_focus_with_it`.
//!
//! **And what the pointer is over is asked again where it lands** (#229): a
//! window still sliding into place when the warp was carried out is given the
//! pointer, and says its cursor, when the picture lands.
//! `tests::real_client::follow::where_the_picture_lands_the_pointer_is_given_to_what_is_there`.

use std::time::Duration;

use smithay::{
    input::pointer::{MotionEvent, PointerHandle},
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::pointer_constraints::with_pointer_constraint,
};

use crate::{
    cursor::flight::Flight,
    qml::hosted::PointerKind,
    script::{Interrupt, Warp, WarpTo},
    state::Solium,
};

/// Carry out a warp a script asked for. Called once the dispatch that asked
/// is applied whole; see `Solium::apply`. One asked for while a press is
/// being handled waits for the button to come up, [`released`].
pub(crate) fn warp(state: &mut Solium, warp: Warp) {
    if state.pressing {
        tracing::debug!("a warp during a press waits for the button to come up");
        state.warp_after_press = Some(warp);
        return;
    }
    if state.lock.is_some() {
        tracing::debug!("a warp behind the lock was refused");
        return;
    }
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    if pointer.is_grabbed() || state.hosted_grab.is_some() || state.scene_press.is_some() {
        tracing::debug!("a warp while the pointer is held was refused");
        return;
    }
    if held_by_a_client(&pointer) {
        tracing::debug!("a warp while a client holds the pointer was refused");
        return;
    }
    let Some(wanted) = state.warp_target(warp.to) else {
        tracing::debug!(?warp, "a warp to nothing that is there");
        return;
    };
    let screens = state.screens();
    if screens.is_empty() {
        return;
    }
    let location = super::confine(&screens, wanted);
    let now = state.clock.now();
    let from = state.drawn_pointer_at(now);
    place(state, &pointer, location);
    state.flight = warp
        .motion
        .map(|motion| Flight::new(from, location, now, motion, warp.interrupt))
        .filter(|flight| !flight.landed(now) && flight.from() != flight.to());
    state.redraw = true;
}

/// Real motion arrived: the flight is over, and the hand on the mouse carries
/// on from where the pointer is, which is where the warp put it, unless the
/// flight's `interrupt` is [`Interrupt::Picture`]. Returns where the picture
/// had got to then, for motion that is relative to it, and `None` with no
/// flight or one that carries on from the pointer.
/// `tests::real_client::follow::real_motion_mid_flight_wins_at_once`,
/// `tests::real_client::follow::with_focus_following_the_mouse_a_nudge_mid_flight_keeps_the_keyboard_where_the_key_put_it`.
pub(crate) fn interrupt(state: &mut Solium) -> Option<Point<f64, Logical>> {
    let flight = state.flight.take()?;
    state.redraw = true;
    match flight.interrupt() {
        Interrupt::Pointer => None,
        Interrupt::Picture => Some(flight.at(state.clock.now())),
    }
}

/// A button came up: a warp that waited for it is carried out once every
/// button is up, as any warp is -- so it is refused, and forgotten, if
/// something still holds the pointer then, such as a menu the press opened.
/// `tests::real_client::follow::a_click_that_takes_the_pointer_along_lands_where_it_was_aimed`,
/// `tests::real_client::follow::a_click_that_opened_a_menu_takes_the_pointer_nowhere_later`.
pub(crate) fn released(state: &mut Solium) {
    if state.pointer_buttons != 0 {
        return;
    }
    if let Some(waited) = state.warp_after_press.take() {
        warp(state, waited);
    }
}

/// A press: the picture lands where the pointer is, which is where the press
/// goes. `tests::real_client::follow::a_press_mid_flight_lands_it`.
pub(crate) fn land(state: &mut Solium) {
    if state.flight.take().is_some() {
        state.redraw = true;
    }
}

/// Whether a client holds the pointer: a locked or confined pointer it was
/// granted. A game that locked the pointer to turn its camera keeps it.
/// `tests::real_client::follow::a_held_pointer_is_not_warped`.
pub(crate) fn held_by_a_client(pointer: &PointerHandle<Solium>) -> bool {
    let Some(surface) = pointer.current_focus() else {
        return false;
    };
    with_pointer_constraint(&surface, pointer, |constraint| {
        constraint.is_some_and(|constraint| constraint.is_active())
    })
}

/// The pointer, put at `location` as motion would put it, everything that
/// listens to motion told, and no focus followed: the scenes and the frames
/// hover, the compositor's cursor over its chrome is asserted again, and the
/// client under it is given the pointer and says its own.
fn place(state: &mut Solium, pointer: &PointerHandle<Solium>, location: Point<f64, Logical>) {
    // No motion of the mouse's led here: the next real report starts a
    // movement of its own.
    // `tests::real_client::follow::real_motion_after_a_warp_reports_only_its_own_speed`.
    state.pointer.warped();
    let grabbed = pointer.is_grabbed();
    if state.lock.is_none() {
        let motion = super::scene_event(state, PointerKind::Motion);
        let above = state.grab_pointer(location, motion)
            || state.surface_pointer(true, location, Some(motion));
        super::hover_frame(state, location);
        if !above {
            state.surface_pointer(false, location, Some(motion));
        }
        state.finish_scene_motion();
    }
    let under = state.surface_under(location);
    super::release_cursor(state, under.is_none(), grabbed);
    state.assert_cursor(location, grabbed);
    let time = u32::try_from(state.clock.now().as_millis()).unwrap_or(u32::MAX);
    pointer.motion(
        state,
        under,
        &MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    pointer.frame(state);
    state.redraw = true;
}

impl Solium {
    /// Where the pointer is drawn at `now`: where it is, or where its picture
    /// has got to on a flight there. A frame asks at the instant it sampled,
    /// as it places everything else it draws.
    /// `tests::real_client::follow::a_key_takes_the_pointer_to_the_window_it_focuses_at_once_and_flies_its_picture`.
    pub(crate) fn drawn_pointer_at(&self, now: Duration) -> Point<f64, Logical> {
        let at = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default();
        self.flight
            .as_ref()
            .filter(|flight| !flight.landed(now))
            .map_or(at, |flight| flight.at(now))
    }

    /// Retire a flight that has landed, and ask again what the pointer is
    /// over where it landed (#229). Returns whether one is still under way,
    /// so the frames keep coming while it is.
    /// `tests::real_client::follow::where_the_picture_lands_the_pointer_is_given_to_what_is_there`.
    pub(crate) fn settle_flight(&mut self, now: Duration) -> bool {
        let Some(flight) = self.flight else {
            return false;
        };
        if !flight.landed(now) {
            return true;
        }
        self.flight = None;
        self.redraw = true;
        if let Some(pointer) = self.seat.get_pointer()
            && !pointer.is_grabbed()
            && self.hosted_grab.is_none()
            && self.scene_press.is_none()
            && self.lock.is_none()
        {
            let location = pointer.current_location();
            place(self, &pointer, location);
        }
        false
    }

    /// Where a warp to `to` puts the pointer, before it is kept on a screen:
    /// a point as it is, and a window at its share of the rectangle it is
    /// headed for, once every transform on it has landed -- a desk sliding in
    /// is where it slides to, and a window a layout has just moved is where
    /// it was moved. `None` for a window that is not there, or whose client
    /// has gone.
    /// `tests::real_client::follow::a_warp_goes_where_the_window_was_moved_in_the_same_handler`.
    pub(crate) fn warp_target(&self, to: WarpTo) -> Option<Point<f64, Logical>> {
        match to {
            WarpTo::Point { x, y } => Some((x, y).into()),
            WarpTo::Window { id, at } => {
                let rect = self.headed_for(id)?;
                Some(
                    (
                        rect.loc.x + rect.size.w * at.0,
                        rect.loc.y + rect.size.h * at.1,
                    )
                        .into(),
                )
            }
        }
    }
}
