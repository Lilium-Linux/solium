//! The X server's own input focus, kept on the X11 window that has the
//! keyboard (#399).
//!
//! The seat gives the keyboard to a window's `wl_surface`, and for an X11
//! window that reaches XWayland as `wl_keyboard.enter`. That is half of what
//! the window needs. XWayland passes the keys on through the X server, and the
//! X server's input focus is the window manager's to set: until it is, it
//! stays `PointerRoot`, keys go to whichever X window is under the pointer,
//! and no window is ever told `FocusIn` for the focus itself -- only the
//! `NotifyPointer` kind, which toolkits ignore. So an X11 application never
//! learns it has the keyboard, and whatever it does only with the keyboard
//! goes undone. SDL is the one that hurts: its relative mouse mode applies
//! only to the window with the keyboard, so a game turns its camera with raw
//! motion that XWayland delivers and SDL throws away. Clicks still land, since
//! they need no focus, and that is exactly what Counter-Strike 2 showed.
//!
//! **ICCCM §4.1.7 says how a window is given the focus.** Its `WM_HINTS.input`
//! says whether the window manager may set the focus on it, and
//! `WM_TAKE_FOCUS` in its `WM_PROTOCOLS` says whether it wants to be asked to
//! take it itself; [`Model`] is the four combinations. smithay does exactly
//! this for a seat whose keyboard focus is an `X11Surface`, but Solium's is a
//! `wl_surface`, for X11 and Wayland windows alike, so smithay never does it
//! here, and its window manager's connection is its own. This is a connection
//! of ours, as `cursor::x11` has one, on a thread of its own: a focus change
//! is a round trip or two, and the compositor's loop waits for none of them.
//!
//! The keyboard going anywhere that is not an X11 window takes the X focus to
//! `None`, as smithay does when an `X11Surface` loses it: no X window types
//! while a Wayland one has the keyboard, and the window that had it hears
//! `FocusOut`, so a game lets go of the mouse when the shell takes the
//! keyboard.

use std::sync::mpsc;

use anyhow::{Context as _, Result};
use x11rb::{
    connection::Connection as _,
    errors::{ConnectionError, ReplyError},
    properties::WmHints,
    protocol::xproto::{
        Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, InputFocus, Window,
    },
    rust_connection::RustConnection,
};

/// How an X11 window takes the keyboard, by ICCCM §4.1.7's names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Model {
    /// Takes no keyboard at all: neither may the focus be set on it, nor will
    /// it take it when asked.
    NoInput,
    /// The window manager sets the focus on it. Most windows.
    Passive,
    /// The window manager sets the focus on it, and tells it so with
    /// `WM_TAKE_FOCUS` too, so it can move the focus between its own windows.
    LocallyActive,
    /// Asked with `WM_TAKE_FOCUS`, and sets the focus itself.
    GloballyActive,
}

impl Model {
    /// The model `WM_HINTS.input` and the presence of `WM_TAKE_FOCUS` make.
    /// A window that set no `input` hint is one the focus may be set on:
    /// that is ICCCM's default, and every toolkit's.
    /// `tests::the_four_icccm_models_from_the_hint_and_the_protocol`.
    pub(crate) fn of(input: Option<bool>, take_focus: bool) -> Self {
        match (input.unwrap_or(true), take_focus) {
            (false, false) => Self::NoInput,
            (true, false) => Self::Passive,
            (true, true) => Self::LocallyActive,
            (false, true) => Self::GloballyActive,
        }
    }

    /// Whether the window manager sets the focus on the window itself.
    pub(crate) fn sets_focus(self) -> bool {
        matches!(self, Self::Passive | Self::LocallyActive)
    }

    /// Whether the window is sent `WM_TAKE_FOCUS`.
    pub(crate) fn asks(self) -> bool {
        matches!(self, Self::LocallyActive | Self::GloballyActive)
    }
}

/// The compositor's end: where the window with the keyboard is said.
#[derive(Debug)]
pub(crate) struct Focus {
    sender: mpsc::Sender<Option<Window>>,
}

impl Focus {
    /// Start the connection to XWayland on display `display` that keeps its
    /// focus. `None`, said once, when there is no thread for it: X11 windows
    /// then keep the `PointerRoot` focus they had before this existed.
    pub(crate) fn start(display: u32) -> Option<Self> {
        let (sender, windows) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("solium-x11-focus".to_owned())
            .spawn(move || {
                if let Err(err) = keep(display, &windows) {
                    tracing::warn!(
                        ?err,
                        "stopped keeping the X11 input focus; X11 windows may not \
                         know they have the keyboard"
                    );
                }
            });
        match spawned {
            Ok(_) => Some(Self { sender }),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "no thread to keep the X11 input focus; X11 windows may not \
                     know they have the keyboard"
                );
                None
            }
        }
    }

    /// The keyboard is now on X11 window `window`, or on nothing of X11's.
    pub(crate) fn give(&self, window: Option<Window>) {
        // Sending fails only once the thread has stopped, which it has said.
        let _ = self.sender.send(window);
    }
}

/// The connection's whole life: give the X focus to each window it is told
/// of, until the compositor stops telling or XWayland goes.
fn keep(display: u32, windows: &mpsc::Receiver<Option<Window>>) -> Result<()> {
    let (conn, _) = x11rb::connect(Some(&format!(":{display}")))?;
    let atom = |name: &[u8]| -> Result<Atom> {
        Ok(conn
            .intern_atom(false, name)?
            .reply()
            .context("interning an atom")?
            .atom)
    };
    let protocols = atom(b"WM_PROTOCOLS")?;
    let take_focus = atom(b"WM_TAKE_FOCUS")?;

    while let Ok(mut window) = windows.recv() {
        // Only the last of a burst matters: the focus is a place, not a
        // sequence of them.
        while let Ok(next) = windows.try_recv() {
            window = next;
        }
        let given = match window {
            Some(window) => give(&conn, window, protocols, take_focus),
            None => conn
                .set_input_focus(InputFocus::NONE, x11rb::NONE, x11rb::CURRENT_TIME)
                .map_err(ReplyError::from)
                .and_then(|cookie| cookie.check()),
        };
        match given {
            Ok(()) => {}
            // A window gone between the keyboard reaching it and this, or one
            // not viewable yet: the X server refuses it, and the next change
            // of focus is still made.
            Err(ReplyError::X11Error(err)) => {
                tracing::debug!(?err, window, "the X server refused the input focus");
            }
            // XWayland has gone, which at the end of a session is every time.
            Err(ReplyError::ConnectionError(err)) => {
                tracing::debug!(?err, "XWayland closed the connection that keeps its focus");
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Give X11 window `window` the focus the way its [`Model`] asks.
fn give(
    conn: &RustConnection,
    window: Window,
    protocols: Atom,
    take_focus: Atom,
) -> std::result::Result<(), ReplyError> {
    // A `WM_HINTS` the client wrote wrong is no hint, which is ICCCM's
    // default, rather than the end of the connection that x11rb reports a
    // reply it cannot read as.
    let input = match WmHints::get(conn, window)?.reply() {
        Ok(hints) => hints.and_then(|hints| hints.input),
        Err(ReplyError::ConnectionError(ConnectionError::ParseError(_))) => None,
        Err(err) => return Err(err),
    };
    let asked = conn
        .get_property(false, window, protocols, AtomEnum::ATOM, 0, u32::MAX)?
        .reply()?
        .value32()
        .is_some_and(|mut atoms| atoms.any(|atom| atom == take_focus));
    let model = Model::of(input, asked);
    if model.sets_focus() {
        // Reverting to nothing, as smithay does: a focused window that goes
        // away leaves the keyboard with no X window rather than with a
        // neighbour the compositor did not choose.
        conn.set_input_focus(InputFocus::NONE, window, x11rb::CURRENT_TIME)?
            .check()?;
    }
    if model.asks() {
        let event = ClientMessageEvent::new(
            32,
            window,
            protocols,
            [take_focus, x11rb::CURRENT_TIME, 0, 0, 0],
        );
        conn.send_event(false, window, EventMask::NO_EVENT, event)?
            .check()?;
    }
    conn.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Model;

    /// **The four ICCCM models from the hint and the protocol**: §4.1.7's
    /// table, with no `input` hint read as `True`.
    #[test]
    fn the_four_icccm_models_from_the_hint_and_the_protocol() {
        let models = [
            (Some(false), false),
            (Some(true), false),
            (None, false),
            (Some(true), true),
            (None, true),
            (Some(false), true),
        ]
        .map(|(input, take)| {
            let model = Model::of(input, take);
            (model, model.sets_focus(), model.asks())
        });
        assert_eq!(
            models,
            [
                (Model::NoInput, false, false),
                (Model::Passive, true, false),
                (Model::Passive, true, false),
                (Model::LocallyActive, true, true),
                (Model::LocallyActive, true, true),
                (Model::GloballyActive, false, true),
            ],
            "(the model, whether the focus is set, whether WM_TAKE_FOCUS is sent)"
        );
    }
}
