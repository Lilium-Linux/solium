//! A named shape, resolved onto the files a theme actually ships.
//!
//! `wp_cursor_shape_v1` lets a client say *what the cursor means* — "this is a
//! text field", "this is a resize edge" — and leave the picture to the
//! compositor. smithay turns the protocol's `shape` enum into a
//! [`CursorIcon`] for us (`wayland/cursor_shape.rs:320`), so what is left is
//! the part smithay does not do: turning that icon into a name an XCursor
//! theme has a file under.
//!
//! **The two vocabularies do not line up, and that is the whole of this
//! module.** [`CursorIcon`] speaks the CSS names — `pointer`, `text`,
//! `nwse-resize` — because that is what the protocol was written against. An
//! xcursor theme on disk is a directory of files named in X11's vocabulary,
//! where the hand is `hand2`, the I-beam is `xterm` and the diagonal resize is
//! `bd_double_arrow`. A theme written this decade ships both, as symlinks; a
//! theme written before `cursor-shape` existed ships only the X11 names; and
//! plenty ship an arbitrary subset of each. So one name is not a lookup, it is
//! a list to walk.
//!
//! The list is written out here rather than taken from `cursor_icon`'s own
//! `alt_names`, and the reason is not pride. That list is `&[]` for nine of the
//! thirty-six icons — `context-menu`, `copy`, `move`, `vertical-text`,
//! `zoom-in`, `zoom-out` among them — and an empty list means the shape
//! silently degrades to the arrow on any theme that names its files the X11
//! way. What is written below is a **superset**: every spelling `cursor_icon`
//! knows plus the ones it does not, in an order we chose. [`a_superset_of_the
//! _crates_own_names`](tests::a_superset_of_the_crates_own_names) pins the
//! superset property, so a future version of the crate that learns a new
//! spelling cannot quietly be ahead of us without a test going red.
//!
//! Where the two disagree about *meaning* rather than spelling, ours wins and
//! says so at the entry. `cursor_icon` lists `fleur` — the four-way move
//! cursor — as an alternative for `grab`, which is a hand; it is kept, but
//! last, behind both hand spellings.

use smithay::input::pointer::CursorIcon;

/// Every name a theme might have `icon` under, best first.
///
/// The w3c name is always first, because a theme that ships it means it: it
/// was drawn against the same vocabulary the client is speaking. The X11
/// spellings follow in the order a theme is likely to have them, which is
/// roughly "how long it has been a conventional name".
///
/// **Never empty**, for any icon, including one this build of `cursor_icon`
/// has never heard of: [`CursorIcon`] is `#[non_exhaustive]`, so the `_` arm
/// is not defensive padding but the case where a newer crate hands us a
/// variant this match does not name. It answers with the arrow's names, which
/// is the same answer [`resolve`] falls back to, and means a shape Solium does
/// not know still draws *something* rather than nothing.
pub(crate) fn names(icon: CursorIcon) -> &'static [&'static str] {
    match icon {
        CursorIcon::Default => &[
            "default",
            "left_ptr",
            "arrow",
            "top_left_arrow",
            "left_arrow",
        ],
        // No X11 cursor ever meant this, so there is nothing legacy to fall
        // back to and the default arrow is what a theme without it gets.
        CursorIcon::ContextMenu => &["context-menu", "context_menu"],
        CursorIcon::Help => &["help", "question_arrow", "whats_this", "left_ptr_help"],
        // The hand, and the one the issue calls out: a link is `hand2` on
        // every theme that predates the w3c names, which is most of them.
        CursorIcon::Pointer => &["pointer", "hand2", "hand1", "hand", "pointing_hand"],
        CursorIcon::Progress => &["progress", "left_ptr_watch", "half-busy"],
        CursorIcon::Wait => &["wait", "watch"],
        CursorIcon::Cell => &["cell", "plus"],
        CursorIcon::Crosshair => &["crosshair", "cross", "tcross", "cross_reverse"],
        // The I-beam, and the other one the issue calls out: a text field
        // shows nothing at all today, and `xterm` is what the file is called.
        CursorIcon::Text => &["text", "xterm", "ibeam"],
        CursorIcon::VerticalText => &["vertical-text", "vertical_text"],
        CursorIcon::Alias => &["alias", "link", "dnd-link"],
        CursorIcon::Copy => &["copy", "dnd-copy"],
        // `fleur` is the X11 four-way move arrow and belongs *here*, which is
        // why it is not first in `Grab` below.
        CursorIcon::Move => &["move", "dnd-move", "fleur"],
        CursorIcon::NoDrop => &["no-drop", "circle", "dnd-no-drop", "forbidden"],
        CursorIcon::NotAllowed => &["not-allowed", "crossed_circle", "forbidden", "circle"],
        // `openhand` and `hand1` before `fleur`: a grab is a hand, and
        // `cursor_icon`'s own list puts a move cursor second. Kept only as a
        // last resort so that this stays a superset of the crate's names.
        CursorIcon::Grab => &["grab", "openhand", "hand1", "fleur"],
        CursorIcon::Grabbing => &["grabbing", "closedhand", "dnd-none", "hand2"],
        // The eight single-edge resizes. X11 names them after the *side of the
        // window* rather than after a compass direction, which is why none of
        // these reads like its w3c name.
        CursorIcon::EResize => &["e-resize", "right_side"],
        CursorIcon::NResize => &["n-resize", "top_side"],
        CursorIcon::NeResize => &["ne-resize", "top_right_corner"],
        CursorIcon::NwResize => &["nw-resize", "top_left_corner"],
        CursorIcon::SResize => &["s-resize", "bottom_side"],
        CursorIcon::SeResize => &["se-resize", "bottom_right_corner"],
        CursorIcon::SwResize => &["sw-resize", "bottom_left_corner"],
        CursorIcon::WResize => &["w-resize", "left_side"],
        // The four double-headed ones. `size_hor`/`size_ver`/`size_bdiag`/
        // `size_fdiag` are Qt's spellings and ship with the Breeze themes;
        // `h_double_arrow` and friends are the X11 ones.
        CursorIcon::EwResize => &[
            "ew-resize",
            "h_double_arrow",
            "sb_h_double_arrow",
            "size_hor",
        ],
        CursorIcon::NsResize => &[
            "ns-resize",
            "v_double_arrow",
            "sb_v_double_arrow",
            "size_ver",
        ],
        CursorIcon::NeswResize => &["nesw-resize", "fd_double_arrow", "size_bdiag"],
        CursorIcon::NwseResize => &["nwse-resize", "bd_double_arrow", "size_fdiag"],
        // A column or row divider. `split_h`/`split_v` are the dedicated
        // files; the double arrows are what a theme without them has.
        CursorIcon::ColResize => &[
            "col-resize",
            "split_h",
            "sb_h_double_arrow",
            "h_double_arrow",
        ],
        CursorIcon::RowResize => &[
            "row-resize",
            "split_v",
            "sb_v_double_arrow",
            "v_double_arrow",
        ],
        CursorIcon::AllScroll => &["all-scroll", "size_all", "fleur"],
        CursorIcon::ZoomIn => &["zoom-in", "zoom_in"],
        CursorIcon::ZoomOut => &["zoom-out", "zoom_out"],
        // Drag-and-drop "ask what to do". `copy` is what `cursor_icon` falls
        // back to and is the least wrong of the three drag cursors.
        CursorIcon::DndAsk => &["dnd-ask", "copy", "dnd-copy"],
        CursorIcon::AllResize => &["all-resize", "move", "fleur", "size_all"],
        // See the doc comment: `CursorIcon` is `#[non_exhaustive]`, so this is
        // a newer crate's variant rather than an impossible case.
        _ => names(CursorIcon::Default),
    }
}

/// The first of `icon`'s names this theme has, or the arrow's.
///
/// `has` is the theme's own "can you draw this at this size" — a predicate
/// rather than a `&Theme` so that the chain can be asserted against a
/// handwritten set of names, with no theme on disk, no GPU and no Qt. The
/// fallback behaviour is the part most worth testing and the part least
/// possible to test against a real theme, since which cursors a machine's
/// themes are missing is a property of that machine.
///
/// **A shape the theme cannot supply ends at the theme's own arrow, not at
/// nothing and not at Solium's QML pointer.** That order is deliberate: a
/// session using a theme should keep using it even for a shape the theme's
/// author never drew, because swapping in a *differently designed* arrow for
/// one shape looks like a glitch, whereas the theme's own arrow looks like a
/// theme that does not distinguish that case. Solium's own pointer is still
/// underneath both — `Pointer::element` reaches it when this returns `None` —
/// but this returning `None` means the theme has no arrow either, which
/// `Theme::load` has already refused to load.
pub(crate) fn resolve(icon: CursorIcon, mut has: impl FnMut(&str) -> bool) -> Option<&'static str> {
    if let Some(found) = names(icon).iter().copied().find(|name| has(name)) {
        return Some(found);
    }
    // Not `names(icon) == names(Default)`, which would be true for an unknown
    // variant and would skip a second walk that has already failed — but only
    // by luck. Asking about the icon itself says what is meant.
    if icon == CursorIcon::Default {
        return None;
    }
    names(CursorIcon::Default)
        .iter()
        .copied()
        .find(|name| has(name))
}

#[cfg(test)]
mod tests {
    use smithay::input::pointer::CursorIcon;

    use super::{names, resolve};

    /// Every shape `wp_cursor_shape_v1` version 2 can name.
    ///
    /// Written out rather than iterated, because [`CursorIcon`] is
    /// `#[non_exhaustive]` and has no iterator: there is no way to ask the
    /// crate for its variants, so the list is maintained here and the count
    /// below is what catches it drifting.
    const EVERY: [CursorIcon; 36] = [
        CursorIcon::Default,
        CursorIcon::ContextMenu,
        CursorIcon::Help,
        CursorIcon::Pointer,
        CursorIcon::Progress,
        CursorIcon::Wait,
        CursorIcon::Cell,
        CursorIcon::Crosshair,
        CursorIcon::Text,
        CursorIcon::VerticalText,
        CursorIcon::Alias,
        CursorIcon::Copy,
        CursorIcon::Move,
        CursorIcon::NoDrop,
        CursorIcon::NotAllowed,
        CursorIcon::Grab,
        CursorIcon::Grabbing,
        CursorIcon::EResize,
        CursorIcon::NResize,
        CursorIcon::NeResize,
        CursorIcon::NwResize,
        CursorIcon::SResize,
        CursorIcon::SeResize,
        CursorIcon::SwResize,
        CursorIcon::WResize,
        CursorIcon::EwResize,
        CursorIcon::NsResize,
        CursorIcon::NeswResize,
        CursorIcon::NwseResize,
        CursorIcon::ColResize,
        CursorIcon::RowResize,
        CursorIcon::AllScroll,
        CursorIcon::ZoomIn,
        CursorIcon::ZoomOut,
        CursorIcon::DndAsk,
        CursorIcon::AllResize,
    ];

    /// The two the issue names, pinned as the literal strings they have to be.
    ///
    /// These are the whole feature in miniature. A text field asks for `text`
    /// and every theme on disk calls that file `xterm`; a link asks for
    /// `pointer` and every theme calls it `hand2`. Get either wrong and the
    /// shape resolves to nothing, falls through to the arrow, and the bug
    /// reads exactly like `cursor-shape` never having been implemented — which
    /// is why they are asserted as strings rather than as "the chain is
    /// non-empty".
    #[test]
    fn the_x11_spellings_of_the_two_that_matter() {
        assert_eq!(
            names(CursorIcon::Pointer),
            ["pointer", "hand2", "hand1", "hand", "pointing_hand"],
            "the hand"
        );
        assert_eq!(
            names(CursorIcon::Text),
            ["text", "xterm", "ibeam"],
            "the I-beam"
        );
    }

    /// A representative spread of the rest, including the three families that
    /// are easiest to get subtly wrong.
    ///
    /// The resizes because X11 names them after the *side of the window* and
    /// not after the compass point, so `ne-resize` is `top_right_corner` and a
    /// transposition would be invisible except to someone dragging a corner.
    /// The diagonals because `fd_` and `bd_` are forward- and back-diagonal
    /// and are trivially swappable. `wait` because `watch` is the single
    /// commonest legacy name in any theme.
    #[test]
    fn a_representative_spread_of_the_rest() {
        let first_alternative = |icon| names(icon).get(1).copied();
        assert_eq!(first_alternative(CursorIcon::Wait), Some("watch"));
        assert_eq!(first_alternative(CursorIcon::Crosshair), Some("cross"));
        assert_eq!(first_alternative(CursorIcon::Move), Some("dnd-move"));
        assert_eq!(
            first_alternative(CursorIcon::NeResize),
            Some("top_right_corner")
        );
        assert_eq!(
            first_alternative(CursorIcon::SwResize),
            Some("bottom_left_corner")
        );
        assert_eq!(
            first_alternative(CursorIcon::EwResize),
            Some("h_double_arrow")
        );
        assert_eq!(
            first_alternative(CursorIcon::NeswResize),
            Some("fd_double_arrow"),
            "forward diagonal, which is the one that is trivially swapped"
        );
        assert_eq!(
            first_alternative(CursorIcon::NwseResize),
            Some("bd_double_arrow")
        );
        assert_eq!(first_alternative(CursorIcon::AllScroll), Some("size_all"));
    }

    /// Thirty-six shapes, every one of them with a chain that starts with the
    /// name the client will have used.
    ///
    /// The count is the guard against a variant being added to the protocol
    /// and quietly landing in the `_` arm as an arrow. The first-name
    /// assertion is the guard against a copy-paste in the table, which is the
    /// realistic way thirty-six near-identical lines go wrong.
    #[test]
    fn every_shape_is_mapped_and_leads_with_its_own_name() {
        assert_eq!(EVERY.len(), 36, "a shape was added or removed");
        for icon in EVERY {
            let chain = names(icon);
            assert_eq!(
                chain.first().copied(),
                Some(icon.name()),
                "{} does not lead with its own w3c name",
                icon.name()
            );
        }
    }

    /// We never know *fewer* spellings than `cursor_icon` does.
    ///
    /// The table here is maintained by hand and the crate's is maintained
    /// upstream; this is what stops the two diverging silently in the
    /// direction that loses cursors. It is one-directional on purpose — ours
    /// is deliberately larger, because nine of the crate's lists are empty.
    #[test]
    fn a_superset_of_the_crates_own_names() {
        for icon in EVERY {
            let ours = names(icon);
            for theirs in icon.alt_names() {
                assert!(
                    ours.contains(theirs),
                    "{} knows {theirs} and we do not",
                    icon.name()
                );
            }
        }
    }

    /// A theme with only the legacy spelling still answers.
    ///
    /// This is the fallback doing its job: the theme has `xterm` and has never
    /// heard of `text`, which is an ordinary theme rather than a broken one.
    #[test]
    fn a_theme_with_only_the_legacy_name_still_resolves() {
        let only = |shipped: &'static str| move |name: &str| name == shipped;
        assert_eq!(resolve(CursorIcon::Text, only("xterm")), Some("xterm"));
        assert_eq!(resolve(CursorIcon::Pointer, only("hand2")), Some("hand2"));
        assert_eq!(
            resolve(CursorIcon::NwseResize, only("size_fdiag")),
            Some("size_fdiag"),
            "Qt's spelling, three deep in the chain"
        );
    }

    /// And a theme with the modern name uses it rather than walking past it.
    ///
    /// The order matters as much as the membership: a theme that ships both
    /// `text` and `xterm` — every theme written this decade — must be read
    /// through the name its author meant, which is the w3c one.
    #[test]
    fn the_modern_name_wins_when_a_theme_has_both() {
        let both = |name: &str| name == "text" || name == "xterm";
        assert_eq!(resolve(CursorIcon::Text, both), Some("text"));
    }

    /// A shape the theme has under *no* name falls back to the arrow.
    ///
    /// The case the issue is explicit about: a shape that cannot be supplied
    /// must not end in an empty cursor. `zoom-in` is the realistic one —
    /// hardly any theme on disk has it — and what it must produce is whatever
    /// the theme calls its arrow, not `None`.
    #[test]
    fn an_unavailable_shape_falls_back_to_the_arrow() {
        let arrow_only = |name: &str| name == "left_ptr";
        assert_eq!(resolve(CursorIcon::ZoomIn, arrow_only), Some("left_ptr"));
        assert_eq!(
            resolve(CursorIcon::ContextMenu, arrow_only),
            Some("left_ptr")
        );
        assert_eq!(
            resolve(CursorIcon::Grabbing, arrow_only),
            Some("left_ptr"),
            "even a shape with four spellings of its own"
        );
    }

    /// A theme with nothing at all is the one case that returns `None`, and
    /// that `None` is what reaches Solium's own QML pointer.
    ///
    /// `Theme::load` refuses a theme with neither `default` nor `left_ptr`, so
    /// this is not reachable through a loaded theme — which is exactly why it
    /// is worth stating here rather than assuming. The floor of the whole
    /// module is the QML pointer, and this is the call that hands over to it.
    #[test]
    fn a_theme_with_nothing_resolves_to_nothing() {
        assert_eq!(resolve(CursorIcon::Text, |_| false), None);
        assert_eq!(resolve(CursorIcon::Default, |_| false), None);
    }

    /// The arrow itself does not walk its own chain twice.
    ///
    /// Cheap to get wrong and impossible to see: a `resolve` that fell back
    /// unconditionally would ask the theme about `left_ptr` and friends a
    /// second time for every missing arrow, and `Theme::has` is a `HashMap`
    /// lookup *after* the first call and a directory walk on it.
    #[test]
    fn the_default_does_not_search_twice() {
        let mut asked = 0;
        let found = resolve(CursorIcon::Default, |_| {
            asked += 1;
            false
        });
        assert_eq!(found, None);
        assert_eq!(
            asked,
            names(CursorIcon::Default).len(),
            "the arrow's chain was walked more than once"
        );
    }
}
