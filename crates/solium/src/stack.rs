//! What is stacked over what on one monitor, above and below the windows
//! (#141, #142).
//!
//! **One order, written once, and read by the renderer and by every hit test
//! above or below the windows.** #137's rule again: `render::drawn_on` and
//! `state::owns` say whether a window is drawn at all and nobody keeps a
//! second copy, and this says what is drawn over what. Before it there were
//! two copies and they disagreed: the frame was built with a scripted `top`
//! surface over a scripted `overlay` one and with a client's layer surfaces in
//! the order they were mapped whatever layer each asked for, while the hit
//! tests asked `overlay` before `top`. What was on top was not what was
//! clicked.
//!
//! [`order`] is the order of the bands; within one, the order is the one each
//! kind of surface already had, and `render::stacked` and `layer::on` are
//! where it is walked.

use crate::scripted::Layer;

/// Whose surfaces a band holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A client's, over `wlr-layer-shell`: Waybar, a launcher, `swaybg`.
    Client,
    /// A script's, through `sol.surface`: the wallpaper, Solium's own shell.
    Script,
}

/// One band of a monitor's stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Band {
    /// One owner's surfaces at one layer.
    Layer(Layer, Owner),
    /// The window [`lifted`] out of the windows, with its popups: a
    /// fullscreen window in front of the monitor's shown workspace.
    Fullscreen,
    /// The windows, and everything each one draws.
    Windows,
}

/// Every band, topmost first.
///
/// **Within each layer the client's surfaces are over the script's**, which is
/// what `scripted::Layer` says: a real bar covers a scripted one and `swaybg`
/// covers the scripted wallpaper, because the client was installed on
/// purpose. The renderer had it the other way round at `top` and at `bottom`.
///
/// And the fullscreen band between `overlay` and `top`, as #142 asks: a
/// fullscreen video covers the bars, and a notification, an OSD or a launcher
/// still shows over it.
///
/// `stacking_is_by_layer_with_the_client_over_the_script` and
/// `a_lifted_window_is_under_overlay_and_over_top`.
const ORDER: [Band; 10] = [
    Band::Layer(Layer::Overlay, Owner::Client),
    Band::Layer(Layer::Overlay, Owner::Script),
    Band::Fullscreen,
    Band::Layer(Layer::Top, Owner::Client),
    Band::Layer(Layer::Top, Owner::Script),
    Band::Windows,
    Band::Layer(Layer::Bottom, Owner::Client),
    Band::Layer(Layer::Bottom, Owner::Script),
    Band::Layer(Layer::Background, Owner::Client),
    Band::Layer(Layer::Background, Owner::Script),
];

/// A monitor's bands, topmost first, with [`Band::Fullscreen`] only when a
/// window is lifted into it -- see [`lifted`].
pub(crate) fn order(lifted: bool) -> impl Iterator<Item = Band> {
    ORDER
        .into_iter()
        .filter(move |band| lifted || *band != Band::Fullscreen)
}

/// The bands above the windows, topmost first.
pub(crate) fn above(lifted: bool) -> impl Iterator<Item = Band> {
    order(lifted).take_while(|band| *band != Band::Windows)
}

/// The bands below the windows, topmost first.
pub(crate) fn below() -> impl Iterator<Item = Band> {
    order(false)
        .skip_while(|band| *band != Band::Windows)
        .skip(1)
}

/// What a fullscreen window covers: `fullscreen.covers` in `config.lua`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Covers {
    /// The top layer, the client's and the script's: the bars go under it.
    #[default]
    Top,
    /// Nothing: the bars stay over a fullscreen window.
    Nothing,
}

impl Covers {
    /// The setting's value, as `config.lua` writes it.
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "top" => Some(Self::Top),
            "none" => Some(Self::Nothing),
            _ => None,
        }
    }
}

/// What [`lifted`] asks of one pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// On the monitor's shown workspace: drawn on that monitor, and not on a
    /// workspace a selection is putting away -- what is left of a window
    /// whose client has gone included, for as long as it is drawn. See
    /// `Solium::lifted_on`.
    pub(crate) shown: bool,
    /// Its client is fullscreen.
    pub(crate) fullscreen: bool,
}

/// The pane lifted into [`Band::Fullscreen`], if any: the front pane on the
/// monitor's shown workspace, when it is fullscreen and `covers` says so.
///
/// `front_first` is the monitor's panes, the front one first. Only the front
/// shown one is asked about: a fullscreen window behind another on its own
/// workspace is not what the user is looking at, and one on a workspace that
/// is not shown changes nothing.
/// `only_the_front_pane_of_the_shown_workspace_is_lifted`.
pub(crate) fn lifted<T>(
    front_first: impl IntoIterator<Item = (T, Candidate)>,
    covers: Covers,
) -> Option<T> {
    if covers == Covers::Nothing {
        return None;
    }
    let (pane, front) = front_first
        .into_iter()
        .find(|(_, candidate)| candidate.shown)?;
    front.fullscreen.then_some(pane)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn client(layer: Layer) -> Band {
        Band::Layer(layer, Owner::Client)
    }

    const fn script(layer: Layer) -> Band {
        Band::Layer(layer, Owner::Script)
    }

    /// **By layer, and the client over the script in every one of them.**
    /// The renderer had a scripted `top` surface over every client one and
    /// over a scripted `overlay` (#141).
    #[test]
    fn stacking_is_by_layer_with_the_client_over_the_script() {
        assert_eq!(
            order(false).collect::<Vec<_>>(),
            vec![
                client(Layer::Overlay),
                script(Layer::Overlay),
                client(Layer::Top),
                script(Layer::Top),
                Band::Windows,
                client(Layer::Bottom),
                script(Layer::Bottom),
                client(Layer::Background),
                script(Layer::Background),
            ]
        );
    }

    /// **A lifted window is under `overlay` and over `top`**, the client's
    /// and the script's (#142).
    #[test]
    fn a_lifted_window_is_under_overlay_and_over_top() {
        let bands: Vec<Band> = order(true).collect();
        let at = |band| bands.iter().position(|each| *each == band);
        assert_eq!(bands.len(), 10, "every band once: {bands:?}");
        assert!(at(script(Layer::Overlay)) < at(Band::Fullscreen));
        assert!(at(Band::Fullscreen) < at(client(Layer::Top)));
        assert!(at(Band::Fullscreen) < at(Band::Windows));
    }

    /// The two halves either side of the windows, which is what the hit tests
    /// walk: everything above, and everything below, and the windows in
    /// neither.
    #[test]
    fn above_and_below_split_the_order_at_the_windows() {
        for lifted in [false, true] {
            let whole: Vec<Band> = order(lifted).collect();
            let halves: Vec<Band> = above(lifted)
                .chain([Band::Windows])
                .chain(below())
                .collect();
            assert_eq!(halves, whole, "lifted = {lifted}");
        }
        assert!(above(true).any(|band| band == Band::Fullscreen));
        assert!(!above(false).any(|band| band == Band::Fullscreen));
    }

    #[test]
    fn covers_is_top_or_none() {
        assert_eq!(Covers::default(), Covers::Top);
        assert_eq!(Covers::named("top"), Some(Covers::Top));
        assert_eq!(Covers::named("none"), Some(Covers::Nothing));
        assert_eq!(Covers::named("Top"), None);
        assert_eq!(Covers::named("bottom"), None);
    }

    /// **The front pane of the shown workspace, and only when it is
    /// fullscreen.** Panes are listed front first, as `Solium::lifted_on`
    /// walks them.
    #[test]
    fn only_the_front_pane_of_the_shown_workspace_is_lifted() {
        let pane = |shown, fullscreen| Candidate { shown, fullscreen };
        assert_eq!(
            lifted([("video", pane(true, true))], Covers::Top),
            Some("video"),
            "a fullscreen window in front is lifted"
        );
        assert_eq!(
            lifted([("video", pane(true, true))], Covers::Nothing),
            None,
            "unless `fullscreen.covers` is \"none\""
        );
        assert_eq!(
            lifted(
                [("parked", pane(false, true)), ("editor", pane(true, false))],
                Covers::Top
            ),
            None,
            "a fullscreen window on a workspace that is not shown changes nothing"
        );
        assert_eq!(
            lifted(
                [("parked", pane(false, false)), ("video", pane(true, true))],
                Covers::Top
            ),
            Some("video"),
            "and a window in front of it on a workspace that is not shown does not \
             stop it being in front of its own"
        );
        assert_eq!(
            lifted(
                [("dialog", pane(true, false)), ("video", pane(true, true))],
                Covers::Top
            ),
            None,
            "a fullscreen window behind another on its own workspace is not lifted"
        );
        assert_eq!(lifted(Vec::<(&str, Candidate)>::new(), Covers::Top), None);
    }
}
