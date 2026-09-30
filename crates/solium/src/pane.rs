//! What the compositor thinks a window is.
//!
//! Not `smithay::desktop::Window`. That type requires a surface, and a window's
//! life begins when the user asks for the application — before there is a
//! process, let alone a surface. Everything that follows from that (the slot
//! reserved immediately, the other windows moving aside, closing it while it is
//! still loading, the application's content appearing *inside* it) is
//! impossible while the compositor's idea of a window is the client's.
//!
//! So: a pane. It owns an identity and a slot, and its *content* is a QML scene
//! the compositor draws, a mapped client, or the remains of one on its way out.
//! The identity and the slot survive all three, which is what makes adoption
//! possible — a client maps into a pane that already exists rather than
//! creating a window beside it.
//!
//! QML is not a special case here. A pane whose content is a scene is as
//! ordinary as one whose content is a client, which is what makes a loading
//! window, a placeholder for an application that died, and a surface the
//! compositor draws for its own reasons one mechanism instead of three.
//!
//! `Space` has not gone away and is not going to. It stays underneath as the
//! authority on stacking and damage for mapped clients, because that
//! bookkeeping is worth keeping and not worth rewriting. `Panes` is the
//! compositor's own view *over* it: everything the compositor decides — which
//! window a script means, what is drawn, what the pointer is over — asks here,
//! and only the mapped case asks `Space` anything.
//!
//! See `docs/spikes/2026-09-06-window-provider.md` for the order the migration
//! goes in and why it goes in that order.
use std::{path::PathBuf, time::Duration};

use smithay::{
    desktop::Window,
    utils::{Logical, Rectangle},
};

/// A pane's identity. Survives adoption, so a script that learned about a
/// window while it was loading is talking about the same window afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PaneId(u64);

impl PaneId {
    /// The next identity. Monotonic and never reused: an id that came back
    /// would let a stale reference address a different window.
    fn next() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// What is inside a pane.
#[derive(Debug)]
pub(crate) enum Content {
    /// Asked for, not arrived. The compositor draws it, from `scene`.
    Loading {
        /// What was asked for, as the user would recognise it.
        program: String,
        /// The process spawned for it, matched against a client's own when it
        /// connects. `None` means nothing will ever be adopted into this pane.
        pid: Option<u32>,
        /// Which QML draws it. Resolved once, so a reload changing the setting
        /// does not change what a pane already on screen looks like halfway.
        #[expect(
            dead_code,
            reason = "written and never read: `Solium::begin_loading` builds the \
                      scene from its own copy before the pane exists, and nothing \
                      rebuilds a scene from this one"
        )]
        source: PathBuf,
        /// The scene itself, hosted by us. `None` if it would not load: a
        /// window with nothing in it is worse than one with a scene, and much
        /// better than no window at all.
        scene: Option<Box<crate::surface::ShellSurface>>,
    },
    /// A client's window, mapped and drawing for itself.
    Client {
        window: Window,
        /// The scene this window was showing before its application arrived,
        /// kept until the application has actually drawn something.
        ///
        /// A client maps a good while before it paints. Dropping the scene the
        /// moment it maps leaves the window empty for exactly that long, which
        /// is the blank flash between the two that this whole design exists to
        /// remove. The two are never both absent: the scene goes when there is
        /// something to replace it with.
        scene: Option<Box<crate::surface::ShellSurface>>,
        /// When the application painted and the scene began to fade off it.
        ///
        /// `None` while the application still has nothing to show. The scene
        /// is drawn *over* the window for as long as this lasts, so the
        /// application is already there underneath as it goes — a dissolve
        /// rather than a cut.
        faded: Option<Duration>,
    },
    /// The remains of a window whose client has gone, on screen for the length
    /// of its fade and not a moment longer (#126). Boxed: it carries two
    /// rectangles, a title, two lists and a picture, and a window that is not
    /// leaving should not pay for them. See [`Left`].
    Leaving(Box<Left>),
}

/// How long a pane whose client has gone stays on screen.
///
/// Exactly the fade `present::close` plays, which is the one a compositor's own
/// close plays too: the pane is at opacity zero from here on, so keeping it any
/// longer keeps a picture of nothing. **Measured from when the fade began and
/// not from when it was noticed**, so a client that goes part of the way
/// through a close the compositor started leaves on that close's schedule
/// (`a_client_that_quits_during_a_close_leaves_on_that_close`). Retired on this instant alone, and never on the animation finishing:
/// `present::close` is written never to release, so "finished" is not a thing
/// anyone can observe. `Solium::settle_leaving` and `Panes::sync` both retire
/// on it; `a_window_that_closes_itself_fades_out_and_is_gone_on_time` pins the
/// first and `syncing_retires_a_pane_that_has_finished_leaving` the second.
pub(crate) const LEAVING: Duration = crate::present::CLOSING;

/// What a pane whose client has gone shows while it fades.
#[derive(Debug)]
pub(crate) enum Remains {
    /// The client's own surfaces, from the textures the renderer had imported
    /// for them. See [`crate::remains`].
    Picture(crate::remains::Picture),
    /// The compositor's own scene: a window whose application never arrived,
    /// or one that mapped and never painted, where the scene was what stood on
    /// screen. It fades as it would have faded under a close.
    Scene(Box<crate::surface::ShellSurface>),
    /// A window that was on screen and left nothing to draw it from. Its
    /// frame, if it has one, and [`crate::remains::FILL`] where its client was.
    Lost,
}

/// Everything a pane keeps of a window whose client has gone.
///
/// **Taken before anything moves**, which is the whole reason this exists as
/// a value rather than being asked of the pane each frame. `Solium::pane_outer`
/// falls back to the pane's slot once there is no client to ask, and the
/// `close` that follows the capture lets a layout move the pane's neighbours --
/// so every fact about where and how the window was drawn is read while it
/// still has a client, and kept. `Solium::depart` is the one writer.
#[derive(Debug)]
pub(crate) struct Left {
    /// When its fade began. See [`LEAVING`].
    pub(crate) since: Duration,
    /// Its outer rectangle, frame included: what its transform is expressed
    /// against, as `Solium::pane_outer` answered while there was a client.
    pub(crate) outer: Rectangle<i32, Logical>,
    /// The client's share of it, as `Solium::pane_geometry` answered.
    pub(crate) geometry: Rectangle<i32, Logical>,
    /// The selections it was in when its client went, by name, whose shift it
    /// is drawn under for as long as it fades -- the shift they have *now*, so
    /// a window that goes during a workspace slide, or just before one, goes on
    /// moving with its desk rather than stopping where the slide had it.
    ///
    /// **The names and not the members**, because a group's members are a
    /// script's to declare, and `workspaces.lua` rebuilds a desk's membership
    /// from `sol.windows()` -- which this pane is no longer in -- so asking the
    /// groups which ones hold it would lose the desk's shift on the next
    /// `layout` event. `a_window_that_left_keeps_the_shift_its_desk_had` pins
    /// the rebuild and `a_window_that_left_moves_with_its_desk` the desk moving
    /// afterwards. A selection forgotten by name while it fades stops carrying
    /// it, as it stops carrying every member -- read, not tested; the shipped
    /// scripts forget a desk only when the number of desks goes down.
    pub(crate) groups: Vec<Box<str>>,
    /// The panes it is drawn over, which is where it stays in the stack while
    /// it fades: [`Panes::sync`] puts it directly above whichever of them is
    /// highest, wherever that one has been raised to since.
    ///
    /// Every pane stacked under it when its client went, and every one the
    /// layout grew into its space at `close`. So a window that goes behind
    /// another one fades behind it -- a terminal behind a browser does not
    /// jump in front of it to fade
    /// (`a_window_that_closes_itself_behind_another_fades_behind_it`); one that
    /// goes from the top stays over the window the keyboard moves to, although
    /// focusing that window raises it
    /// (`a_window_that_goes_from_the_top_stays_over_the_window_the_keyboard_moves_to`);
    /// and the neighbour a layout grows into its place grows in behind the
    /// fade, as it does behind a close the compositor asks for (#128), which
    /// raises the window at the press
    /// (`a_window_that_closes_itself_hands_its_space_over_as_it_fades`).
    ///
    /// **What that costs**: the rule cannot tell a raise that is the keyboard
    /// moving on from one that is a click, so a window it was over that is
    /// clicked to the front inside those 190 ms takes the fade up with it, over
    /// whatever was covering both. Read, not tested.
    pub(crate) over: Vec<PaneId>,
    /// What its frame said, and whether it was drawn focused, so a titlebar
    /// fades out as it stood rather than losing its title on the way. Both
    /// read back in `a_window_that_left_is_nobodys_to_find`.
    pub(crate) title: String,
    pub(crate) focused: bool,
    /// What it is drawn from.
    pub(crate) remains: Remains,
    /// The fill's identity for the damage tracker, for [`Remains::Lost`].
    pub(crate) fill: smithay::backend::renderer::element::Id,
}

/// What the compositor draws around this pane's client.
///
/// One value rather than two tables. `Decorations` held `frames:
/// HashMap<PaneId, Decoration>` and `bare: HashSet<PaneId>` as parallel
/// collections keyed by `PaneId`, and two tables answering one question can
/// disagree — `Solium::insets_of` checked `frames` first, so a pane in both had
/// `bare` silently ignored, and nothing tested that. There is nowhere left for
/// that state to be: three arms, one value, and it belongs to the pane it is
/// drawn around.
///
/// **Every reader asks this**, whether it wants a fact about the frame — how
/// much room it takes, whether there is one at all — or the live scene itself.
/// `Decorations` is down to the style a script chose and the code that builds a
/// [`crate::decoration::Decoration`] from it; what it builds it hands to the
/// pane. See "Pane ownership" in `docs/design/2026-09-08-pane-styles-design.md`.
///
/// **There is no `large_enum_variant` suppression on this any more, and its
/// absence is the measurement.** There was one, justified by
/// `size_of::<Decoration>()` being 248; a decoration then became a *list* of
/// layers, its scene and its backing moved behind that list's pointer, and it
/// is now **112 bytes** — under clippy's 200-byte threshold, so the lint does
/// not fire at all and an `#[expect]` for it is an unfulfilled expectation and
/// a warning of its own. The niche is unchanged and still load-bearing, which
/// is what `a_frame_costs_what_the_decoration_in_it_costs` pins: a `Vec`'s
/// pointer is non-null, so the discriminant still lands inside the payload and
/// `Frame` is 112 as well. `Pane` is 712.
///
/// Those three literals have gone stale once already — bleed put the pane's
/// size into `Shown`, eight bytes, and all three moved together. They are here
/// to give the threshold a scale and nothing depends on them, which is why the
/// test is a relation and not two numbers.
#[derive(Debug, Default)]
pub(crate) enum Frame {
    /// A client is still coming, or its frame has not been built yet. Keep
    /// reserving the insets a frame will want, or the window jumps when it
    /// finally arrives.
    #[default]
    Pending,
    /// There will never be a frame: the client draws its own, or this is an
    /// override-redirect menu. Not the same as "not yet".
    None,
    /// Drawn by the compositor, from this scene.
    ///
    /// The [`crate::decoration::Decoration`] itself, owned here and nowhere
    /// else: it is a live Qt scene, is not `Clone`, and there is exactly one
    /// per decorated window. So it is **moved** in — a second one would be two
    /// scenes per window, which is a behaviour change wearing a refactor's
    /// clothes — and it leaves when the pane does, which is the table
    /// reconciliation this whole change exists to delete.
    ///
    /// Not boxed. Measured: `size_of::<Decoration>()` is 112 and `Frame` with
    /// this arm inline is also 112, because the discriminant lands in a niche.
    /// A `Box` here buys an allocation per decorated window and saves nothing —
    /// and a decoration's own scenes are already behind one pointer, since a
    /// style is a `Vec` of layers.
    Styled(crate::decoration::Decoration),
}

/// The two tiles a pane can hold -- the one it is in and the one it left for a
/// maximise -- as [`Pane::take_let_go`] takes them out together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tiles {
    placed: Option<Rectangle<i32, Logical>>,
    left_tile: Option<Rectangle<i32, Logical>>,
}

/// A window, as the compositor thinks of one.
#[derive(Debug)]
pub(crate) struct Pane {
    id: PaneId,
    slot: Rectangle<i32, Logical>,
    /// The **outer** rectangle of the tile a layout holds this pane in, while
    /// one does.
    ///
    /// **Not a cache of [`Self::slot`], and the difference is the whole reason
    /// this field exists.** `Panes::sync` writes the space's answer over `slot`
    /// on every frame a pane is not under a resize hold, and the space reports
    /// a mapped window's size as whatever the *client* last committed — so the
    /// moment a client answers a configure with a size of its own, `slot` stops
    /// being the rectangle the layout asked for and becomes the layout's origin
    /// paired with the client's size. That is not a rare case: a terminal on a
    /// cell grid does it on every resize it is ever given, and
    /// `Solium::settle_resize_hold` deliberately adopts such an answer into the
    /// slot rather than fighting it.
    ///
    /// Written by `Solium::move_pane`, and by nothing that hears from a
    /// client. That is what makes it the one place the compositor keeps the
    /// *layout's* opinion of where this pane is, which is what an edge drag has
    /// to start from — see `Solium::pane_laid_out` for why the client's opinion
    /// will not do.
    ///
    /// **And it is the tile the client is held inside (#133).**
    /// `Solium::pane_geometry` caps a settled client's committed size at this
    /// rectangle's client share, and `render::elements` cuts the client's
    /// surfaces to it, so a client that will not shrink as far as its tile —
    /// a terminal on its cell grid, a browser at its minimum width — is drawn
    /// inside the tile rather than over its neighbours. That is why this means
    /// "tiled now" and not "tiled once": a stale rectangle here would cut a
    /// maximised, fullscreen or floating window down to the tile it used to
    /// have. So it is cleared on every way out of a tile — a maximise and a
    /// fullscreen (which keep it in [`Self::left_tile`] for the way back), a
    /// `sol.place` with `tile = false`, and `sol.unplace`, which is what
    /// `modes.use` sends for every window when the layout changes.
    /// `a_maximised_window_is_not_held_in_its_old_tile`,
    /// `a_fullscreen_window_is_not_held_in_its_old_tile` and
    /// `a_window_let_go_by_its_layout_is_not_held_in_its_old_tile` are the
    /// three.
    ///
    /// **Every way out but one, and the one is load-bearing: a close (#128).**
    /// A layout's `closing` handler takes the window out of its tree and says
    /// nothing about this field, and nothing in the compositor clears it at a
    /// close either, so a window being closed keeps the tile it is fading in
    /// until it is gone. That is what keeps it cut. `present::close` pins every
    /// frame of the fade to the rectangle the window was closed at, which is a
    /// picture of this tile, and `render::fit` cuts only a pane that is in a
    /// tile. Cleared at `closing`, a client wider than its tile has its whole
    /// buffer scaled into each frame of the fade -- squashed -- and cleared
    /// before `present::close` reads the rectangle, the fade starts at the
    /// width the client committed and spills over the neighbour growing into
    /// the space, which is what #133 removed. Making the paragraph above true
    /// of a close would do one or the other on every close of such a client.
    /// `a_closed_window_is_cut_to_the_tile_it_left_for_the_whole_fade` fails
    /// both ways.
    ///
    /// **And while a window is leaving, a let-go waits for it.** A
    /// `sol.unplace`, or a `sol.place` with `tile = false`, that arrives during
    /// the close -- `modes.use` sends the first for every window, so a mode
    /// switch inside the fade is one -- leaves this set and is owed instead,
    /// in [`Self::let_go`], until the window comes back. See [`Self::untile`].
    ///
    /// **Usually from the rectangle a layout handed `sol.place`, but not
    /// always**: `Solium::rescue_offscreen` reaches `move_pane` too, with a
    /// rectangle it worked out itself to drag a window back onto a screen that
    /// went away. So the invariant is the narrower one — no client ever writes
    /// here — and not "this is what the layout last said". The next sweep puts
    /// the layout's answer back, and a drag begun in between starts from a
    /// rectangle the window really is at, which is the right answer anyway. A
    /// rescue keeps a pane's standing as it found it: a tiled pane is still
    /// tiled at the rectangle it was rescued to
    /// (`a_tiled_window_brought_back_onto_a_screen_is_still_tiled`), and a
    /// floating one is still floating
    /// (`a_floating_window_brought_back_onto_a_screen_is_not_given_a_tile`).
    ///
    /// `None` for a pane no layout holds in a tile — a floating window, a
    /// dialog a layout centres over its parent, or one in the frames between
    /// mapping and the first sweep — where the pane's own rectangle is the
    /// only answer there is. **And for a maximised or fullscreen window only
    /// until the next sweep:** `tiling.apply` places every leaf of its trees,
    /// a script cannot see that a window is maximised, and a maximise takes no
    /// window out of its tree -- so a relayout while one is maximised, which
    /// `unfullscreen_request` and a layer surface arriving each cause, puts it
    /// back here and `move_pane` configures it into the tile. That configure
    /// is what stage did as well; it is read from `tiling.lua` and `move_pane`,
    /// and no test pins it.
    placed: Option<Rectangle<i32, Logical>>,
    /// A layout let this pane out of its tile while it was leaving, and the
    /// let-go is waiting for the window to come back.
    ///
    /// Set by [`Self::untile`] on a pane that is [`Self::leaving`], in place of
    /// clearing [`Self::placed`], because a leaving window is still drawn as it
    /// was closed and its tile is what cuts it -- see that field. So a switch
    /// of mode inside the fade no longer squashes the window fading:
    /// `a_mode_switched_during_a_fade_does_not_squash_the_window_leaving`.
    ///
    /// Taken by `Solium::give_back`, before it aims the window's return, so a
    /// window refused after a switch to floating comes back at the size its
    /// client committed and is not held in the tile it left; the same test
    /// asserts both. Cancelled by a tile a layout gives the window since,
    /// which is the layout's newer word ([`Self::set_placed`]), and kept by a
    /// rescue, which moves the tile and changes nothing else
    /// ([`Self::move_tile`]), which
    /// `a_rescue_during_a_fade_keeps_the_let_go_it_is_waiting_on` drives
    /// through `Solium::rescue_offscreen`. Only `give_back` takes it: a window
    /// that goes is retired with the let-go still owed, and nothing reads it
    /// after that.
    /// `a_let_go_while_leaving_waits_until_the_window_is_back` pins each of
    /// those on one pane.
    let_go: bool,
    /// The tile this pane left when it was maximised or sent fullscreen, kept
    /// for the way back to put it in again.
    ///
    /// Beside [`Self::restore`] and spent with it: written when
    /// `Solium::toggle_maximize` or `Solium::fullscreen_request` takes the
    /// pane out of [`Self::placed`], and taken by whichever of
    /// `toggle_maximize` and `Solium::unfullscreen_request` puts the window
    /// back at its restore rectangle. Without it a window restored into its
    /// tile would be a floating window inside a tiled layout until the next
    /// sweep, and an edge drag begun in between would start from the client's
    /// rectangle rather than the layout's — #124 again, by another door.
    /// `a_restored_window_goes_back_into_its_tile` pins it.
    left_tile: Option<Rectangle<i32, Logical>>,
    /// Where this window goes back to when it leaves maximised or fullscreen.
    ///
    /// Written by `Solium::toggle_maximize` and `Solium::fullscreen_request`
    /// before they move the window, and taken -- used once -- by whichever of
    /// `toggle_maximize` and `Solium::unfullscreen_request` puts it back.
    ///
    /// **One slot for both**, so a window maximised and then sent fullscreen
    /// has the rect from before the maximise here. Leaving fullscreen leaves it
    /// alone for such a window and puts it back to maximised, and ignores a
    /// client asking to leave a fullscreen it is not in; either way round, the
    /// rect would be spent on a window that is still maximised.
    ///
    /// **A property of the window, not of its frame**, and it lived on the
    /// frame until #92. `Solium::fullscreen_request` wrote it into the pane's
    /// `Decoration` and then, two lines later, dropped that decoration so a
    /// fullscreen window has no titlebar; leaving fullscreen built a new one
    /// with this empty, so there was never a rect to put the window back at.
    /// A window with no server-side frame at all -- one that draws its own, or
    /// any window under `pane = "none"` -- had nowhere to keep it in the first
    /// place. For fullscreen that was reachable: a client drawing its own
    /// frame that went fullscreen had no rect kept at all. For maximise it was
    /// latent, because the only way to maximise is a button on a frame, so a
    /// window with no frame had no button to press.
    ///
    /// Here, it survives every [`Self::set_frame`], which is what
    /// `a_windows_way_back_outlives_its_frame` pins.
    restore: Option<Rectangle<i32, Logical>>,
    content: Content,
    /// What is drawn around this pane's client. See [`Frame`].
    ///
    /// A field rather than a lookup, so it leaves with the pane: a frame keyed
    /// by `PaneId` in a table beside the panes has to be reconciled by hand,
    /// and one that is not is kept for ever.
    frame: Frame,
    /// When this pane's close request goes out, once it has finished leaving.
    ///
    /// A *deadline*, not a start: `Solium::close_pane` animates the window
    /// away first and asks the client afterwards, and this is the moment
    /// "afterwards" arrives. `None` for a window nobody has asked to close.
    ///
    /// A field for the reason `frame` is one — `Solium::closing` was a
    /// `HashMap<PaneId, Duration>` beside the panes, and an entry whose pane
    /// had gone stayed until something remembered to sweep it. This one leaves
    /// with its pane.
    closing_at: Option<Duration>,
    /// When this pane's client was asked to close.
    ///
    /// A close is a request. A client may put up "are you sure?" and stay, and
    /// nothing in the protocol says so — the only evidence is the window still
    /// being here a moment later. `None` once that moment has passed and the
    /// window has either gone or been brought back. See
    /// `Solium::settle_refused`.
    asked_at: Option<Duration>,
    /// Whether this close has already been answered, and the window is owed its
    /// place back.
    ///
    /// Set by `Solium::refused_with_a_dialog` and cleared by
    /// `Solium::give_back` on the frame the give-back actually takes. It exists
    /// because a give-back can *decline*: `present::clear` goes through
    /// `with_slot`, which answers `None` rather than panicking when the
    /// transform slot is already borrowed, and on such a frame nothing is
    /// retired. `settle_refused` cannot retry a pane that is still inside
    /// `CLOSING`, where `asked_at` is `None` by definition — so without this
    /// the declined give-back was simply dropped and `settle_closing` went on
    /// to close the parent out from under its own dialog.
    ///
    /// A fact about the close rather than an instant, because there is no
    /// deadline in it: the retry is "every frame until the slot is free", which
    /// is what a busy slot costs everywhere else.
    answered: bool,
    /// Whether scripts have been told this window has gone.
    ///
    /// Set by `Solium::trigger_close`, and never cleared: nothing brings a
    /// window back once `close` has been sent. The pane itself outlives that
    /// call -- as [`Content::Leaving`] for the length of its fade when there is
    /// anything to fade, and otherwise until `sync_panes` retires it at the end
    /// of the frame -- and this is what keeps it from being an ordinary window
    /// meanwhile. See [`Self::leaving`] and `Solium::snapshot`.
    gone: bool,
    opened: Duration,
    /// Whether a client mapped *into* this pane rather than creating it.
    ///
    /// The difference matters exactly once, on that client's first commit: an
    /// adopted pane already has a slot the layout gave it and a layout that
    /// was told it opened, so neither must happen a second time.
    adopted: bool,
    /// Where this pane is *drawn*, when that is not where it lives, and
    /// whether it has ever been on screen. See `present::Slot` — it is here
    /// rather than on the client's window so that it can exist before the
    /// window does, and survive the window arriving.
    drawn: crate::present::Slot,
    /// Whether a layout may place this, and whether it counts as a window.
    ///
    /// False for an X11 override-redirect surface — a menu, a tooltip, a drag
    /// icon. Those say "do not manage me" and they mean it: they place
    /// themselves, they are gone in a moment, and a layout that treats one as
    /// a window reserves a slot for it and reflows the desktop around a
    /// tooltip. Dragging a text selection out of an application did exactly
    /// that.
    ///
    /// They are still panes, because they are on screen and under the pointer
    /// and every path that asks what is on screen asks for panes. What they
    /// are not is *windows*.
    managed: bool,
    /// The texture this pane's warp captures are drawn into, kept across the
    /// frames of an animation. See [`crate::offscreen::Scratch`].
    ///
    /// A field for the reason `frame` and `closing_at` are fields, and with
    /// rather more at stake: a `HashMap<PaneId, GlesTexture>` beside the panes
    /// would be a sixth table reconciled by hand, and what an entry nobody
    /// swept would keep is not a stale boolean but several megabytes of GBM.
    /// This leaves with its pane.
    ///
    /// Empty on a pane nobody is warping, which is all of them on an ordinary
    /// desktop: `render::prepare` hands it back on the first frame a pane is
    /// not captured on.
    scratch: crate::offscreen::Scratch,
    /// The size limits of this pane's client that the layouts were last told
    /// about (#115). Not what the snapshot reads -- that asks the client, so it
    /// is never behind -- but what `Solium::notice_limits` compares with, so
    /// that the layouts are told once for each change and not once for each
    /// commit.
    limits: crate::state::Limits,
    /// Whether the layout that last placed this pane said its tile is smaller
    /// than the window's own minimum. See `WindowInfo::cramped`.
    cramped: bool,
}

impl Pane {
    /// A pane for an application that has been asked for and has not arrived.
    pub(crate) fn loading(
        program: &str,
        pid: Option<u32>,
        slot: Rectangle<i32, Logical>,
        source: PathBuf,
        scene: Option<crate::surface::ShellSurface>,
        now: Duration,
    ) -> Self {
        Self {
            id: PaneId::next(),
            slot,
            placed: None,
            let_go: false,
            left_tile: None,
            restore: None,
            content: Content::Loading {
                program: program.to_owned(),
                pid,
                source,
                scene: scene.map(Box::new),
            },
            frame: Frame::Pending,
            closing_at: None,
            asked_at: None,
            answered: false,
            gone: false,
            opened: now,
            adopted: false,
            drawn: crate::present::Slot::default(),
            managed: true,
            scratch: crate::offscreen::Scratch::default(),
            limits: crate::state::Limits::default(),
            cramped: false,
        }
    }

    /// A pane for a client that arrived without being asked for — anything
    /// started outside the compositor.
    pub(crate) fn mapped(window: Window, slot: Rectangle<i32, Logical>, now: Duration) -> Self {
        Self {
            id: PaneId::next(),
            slot,
            placed: None,
            let_go: false,
            left_tile: None,
            restore: None,
            content: Content::Client {
                window,
                scene: None,
                faded: None,
            },
            frame: Frame::Pending,
            closing_at: None,
            asked_at: None,
            answered: false,
            gone: false,
            opened: now,
            adopted: false,
            drawn: crate::present::Slot::default(),
            managed: true,
            scratch: crate::offscreen::Scratch::default(),
            limits: crate::state::Limits::default(),
            cramped: false,
        }
    }

    /// The client size limits the layouts were last told about. See the
    /// field.
    pub(crate) const fn limits(&self) -> crate::state::Limits {
        self.limits
    }

    /// Record that the layouts have been told these.
    pub(crate) const fn set_limits(&mut self, limits: crate::state::Limits) {
        self.limits = limits;
    }

    /// Whether the layout that last placed this pane said it is cramped.
    pub(crate) const fn cramped(&self) -> bool {
        self.cramped
    }

    /// What the layout placing this pane says about its tile.
    pub(crate) const fn set_cramped(&mut self, cramped: bool) {
        self.cramped = cramped;
    }

    /// Whether a layout may place this pane and count it as a window.
    pub(crate) const fn managed(&self) -> bool {
        self.managed
    }

    /// Mark this pane as one that places itself. See the field.
    pub(crate) const fn unmanage(&mut self) {
        self.managed = false;
    }

    /// How this pane is being drawn. `present` is the only thing that reads it.
    pub(crate) const fn drawn(&self) -> &crate::present::Slot {
        &self.drawn
    }

    pub(crate) const fn id(&self) -> PaneId {
        self.id
    }

    pub(crate) const fn slot(&self) -> Rectangle<i32, Logical> {
        self.slot
    }

    pub(crate) const fn set_slot(&mut self, slot: Rectangle<i32, Logical>) {
        self.slot = slot;
    }

    /// The outer rectangle a layout last asked for this pane. See the field.
    pub(crate) const fn placed(&self) -> Option<Rectangle<i32, Logical>> {
        self.placed
    }

    /// Record the outer rectangle a layout has just asked for this pane.
    ///
    /// Separate from [`Self::set_slot`] rather than folded into it, because the
    /// two have different writers on purpose: every path that hears from a
    /// client sets the slot, and only a layout's sweep sets this.
    ///
    /// **A tile given to a leaving window cancels a let-go it was waiting on**
    /// ([`Self::let_go`]): the layout's newer word is that the window is tiled,
    /// and the window coming back must not be let out of the tile it was just
    /// given.
    pub(crate) const fn set_placed(&mut self, placed: Rectangle<i32, Logical>) {
        self.placed = Some(placed);
        self.let_go = false;
    }

    /// Move the tile this pane is held in, if it is held in one, and change
    /// nothing else about its standing.
    ///
    /// For `Solium::rescue_offscreen`, which brings a window back onto a screen
    /// as whatever it was: a tiled pane still tiled, a floating one still
    /// floating, and a leaving one still owed the let-go it was waiting on --
    /// which [`Self::set_placed`] would cancel.
    pub(crate) const fn move_tile(&mut self, to: Rectangle<i32, Logical>) {
        if self.placed.is_some() {
            self.placed = Some(to);
        }
    }

    /// Take this pane out of its tile for a maximise or a fullscreen, keeping
    /// the tile for the way back. See [`Self::left_tile`].
    ///
    /// A pane that is in no tile keeps whatever tile it was already waiting to
    /// go back to: a window maximised and then sent fullscreen left its tile
    /// at the maximise, and the fullscreen must not forget it. This and the
    /// two below are `a_tile_left_for_a_maximise_is_kept_for_the_way_back_and_no_longer`.
    pub(crate) const fn leave_tile(&mut self) {
        if let Some(placed) = self.placed.take() {
            self.left_tile = Some(placed);
        }
    }

    /// Put this pane back into the tile it left, now that it is back at its
    /// restore rectangle. See [`Self::left_tile`].
    ///
    /// A tile it is in already wins over the kept one: that is the layout's
    /// newer answer. A layout's sweep does not give a maximised or fullscreen
    /// window one: `Solium::move_pane` makes the tile the kept one instead,
    /// `a_layout_leaves_a_fullscreen_or_maximised_window_where_it_is`.
    pub(crate) const fn return_to_tile(&mut self) {
        let left = self.left_tile.take();
        if self.placed.is_none() {
            self.placed = left;
        }
    }

    /// No layout holds this pane in a tile any more, and none is keeping one
    /// for it to go back to.
    ///
    /// Both fields, because a window maximised in tiling and then let go by the
    /// layout — `modes.use` switching to floating — must not be put back in its
    /// old tile by the un-maximise that follows, when there is no layout left
    /// to hold it there.
    ///
    /// **Except for a window that is leaving, which keeps both until it is
    /// back** and owes the let-go instead: see [`Self::let_go`], and
    /// [`Self::placed`] for why a leaving window's tile is load-bearing. A
    /// window that goes never takes it; one that comes back does, in
    /// [`Self::take_let_go`].
    pub(crate) const fn untile(&mut self) {
        if self.leaving() {
            self.let_go = true;
            return;
        }
        self.placed = None;
        self.left_tile = None;
    }

    /// Take the let-go this pane was waiting on, now that it is coming back:
    /// out of both tiles, as [`Self::untile`] would have left it.
    ///
    /// Hands back what it held, so that a give-back which then declines can
    /// owe the let-go again with [`Self::owe_let_go`] and the rest of the fade
    /// is still cut. `None` when nothing was owed, and then nothing changes.
    pub(crate) const fn take_let_go(&mut self) -> Option<Tiles> {
        if !self.let_go {
            return None;
        }
        self.let_go = false;
        Some(Tiles {
            placed: self.placed.take(),
            left_tile: self.left_tile.take(),
        })
    }

    /// Put back what [`Self::take_let_go`] took, still owed.
    pub(crate) const fn owe_let_go(&mut self, held: Tiles) {
        self.placed = held.placed;
        self.left_tile = held.left_tile;
        self.let_go = true;
    }

    /// The tile this pane is waiting to go back into, while it is maximised or
    /// fullscreen. See [`Self::left_tile`]; read by `Solium::pane_laid_out`,
    /// for a drag on the maximised window, and by `Solium::rescue_offscreen`.
    pub(crate) const fn left_tile(&self) -> Option<Rectangle<i32, Logical>> {
        self.left_tile
    }

    /// Move the tile this pane is waiting to go back into, if it is waiting
    /// for one.
    ///
    /// For `Solium::rescue_offscreen`, which drags a maximised window off a
    /// monitor that went away: the tile it left was on that monitor too, and
    /// a restore before the next sweep would put it back there -- where
    /// `Solium::pane_laid_out` would start a drag from, on no screen at all.
    /// `a_maximised_window_brought_back_onto_a_screen_brings_its_tile` pins it.
    pub(crate) const fn move_left_tile(&mut self, to: Rectangle<i32, Logical>) {
        if self.left_tile.is_some() {
            self.left_tile = Some(to);
        }
    }

    /// Where this window goes back to, if a maximise or a fullscreen kept
    /// one. See the field.
    pub(crate) const fn restore(&self) -> Option<Rectangle<i32, Logical>> {
        self.restore
    }

    /// Remember where this window goes back to, or forget it.
    pub(crate) const fn set_restore(&mut self, restore: Option<Rectangle<i32, Logical>>) {
        self.restore = restore;
    }

    /// Where this window goes back to, and forget it: the way back is used
    /// once.
    pub(crate) const fn take_restore(&mut self) -> Option<Rectangle<i32, Logical>> {
        self.restore.take()
    }

    /// What is drawn around this pane's client. See [`Frame`].
    pub(crate) const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// This pane's frame scene, if one has been built.
    pub(crate) const fn decoration(&self) -> Option<&crate::decoration::Decoration> {
        match &self.frame {
            Frame::Styled(decoration) => Some(decoration),
            Frame::Pending | Frame::None => None,
        }
    }

    /// The same, to write to: the readers that want the scene itself rather
    /// than a fact about it — drawing it, giving it the pointer, taking its
    /// button presses.
    ///
    /// The narrowing and not a `frame_mut`, deliberately. Handing out
    /// `&mut Frame` would let any caller swap a `Styled` for a `None` without
    /// going through [`crate::decoration::Decorations`] — a second writer of
    /// the fact this change exists to keep in one place. Everything that
    /// decides *whether* there is a frame goes through `set_frame`.
    pub(crate) const fn decoration_mut(&mut self) -> Option<&mut crate::decoration::Decoration> {
        match &mut self.frame {
            Frame::Styled(decoration) => Some(decoration),
            Frame::Pending | Frame::None => None,
        }
    }

    /// The texture this pane's warp captures are drawn into. See the field and
    /// [`crate::offscreen::Scratch`].
    ///
    /// Only `_mut`, because both things anyone does with it — taking a texture
    /// for a capture, handing one back when the warp ends — write to it. There
    /// is nothing to read.
    pub(crate) const fn scratch_mut(&mut self) -> &mut crate::offscreen::Scratch {
        &mut self.scratch
    }

    /// Say what is drawn around this pane's client.
    ///
    /// `Decorations`' five mutators are the only callers: they own the policy
    /// — which QML, whether the style is `none`, whether it loaded — and this
    /// is where the answer lands. A `Styled` replaced here drops the scene it
    /// held, which is what makes "the frame leaves with its pane" true.
    pub(crate) fn set_frame(&mut self, frame: Frame) {
        self.frame = frame;
    }

    /// When this pane's close request goes out. See the field.
    pub(crate) const fn closing_at(&self) -> Option<Duration> {
        self.closing_at
    }

    /// This pane is on its way out; `due` is when to tell its client so.
    pub(crate) const fn begin_closing(&mut self, due: Duration) {
        self.closing_at = Some(due);
    }

    /// The request has gone out, or there is nothing left to ask.
    pub(crate) const fn stop_closing(&mut self) {
        self.closing_at = None;
    }

    /// When this pane's client was asked to close. See the field.
    pub(crate) const fn asked_at(&self) -> Option<Duration> {
        self.asked_at
    }

    /// The client has been asked to close, at `at`. Whether the request
    /// actually went out is deliberately not recorded: a window we could not
    /// even ask is the one most in need of being waited for.
    pub(crate) const fn mark_asked(&mut self, at: Duration) {
        self.asked_at = Some(at);
    }

    /// Stop waiting on an answer that was never going to come in words.
    pub(crate) const fn forget_asked(&mut self) {
        self.asked_at = None;
    }

    /// Whether this close has been answered and the window is owed its place
    /// back. See the field.
    pub(crate) const fn answered(&self) -> bool {
        self.answered
    }

    /// The client answered this close in as many words, by putting a window on
    /// screen. The give-back is owed from here until it is taken.
    pub(crate) const fn mark_answered(&mut self) {
        self.answered = true;
    }

    /// The give-back took. Nothing is owed.
    pub(crate) const fn settled_answer(&mut self) {
        self.answered = false;
    }

    /// Whether scripts have been told this window has gone. See the field.
    pub(crate) const fn gone(&self) -> bool {
        self.gone
    }

    /// Scripts are being told this window has gone. See the field.
    pub(crate) const fn went(&mut self) {
        self.gone = true;
    }

    /// Whether this pane is on its way off the screen.
    ///
    /// **One question with one answer, because asking half of it is issue
    /// #127.** "On its way out" spans four states and the two callers that
    /// have to know — `Solium::close_pane`, which must not start a second
    /// close, and `Solium::move_pane`, which must not overwrite the transform
    /// that is playing one — each used to ask a different, narrower question.
    /// `close_pane` asked `closing_at().is_some()`, which is false for the
    /// whole of the grace period after the request has gone out, so a second
    /// `super+q` restarted an invisible animation and asked the client to close
    /// a second time. `move_pane` asked nothing at all, and put a dying window
    /// back at full opacity.
    ///
    /// The four states, in the order a window passes through them:
    ///
    /// 1. `closing_at` — the leaving animation is playing and the request has
    ///    not gone out yet. 190 ms.
    /// 2. `asked_at` — the request has gone out and the client has not
    ///    answered. The window is held invisible for as long as this lasts, so
    ///    it is *more* in need of the guard than (1), not less: there is
    ///    nothing on screen for a second press to have been aimed at.
    /// 3. `gone` — scripts have been told `close`, and the pane has not been
    ///    retired yet, which it is at the end of the frame. `trigger_close`
    ///    clears both timers above so that no deadline gives the window back,
    ///    and without this arm that left a window that no longer exists
    ///    answering "not leaving" to both callers for the rest of the frame:
    ///    `close_pane` started a close on it, and `move_pane` drew it again at
    ///    full opacity for a layout placing it in `close`. See
    ///    `a_window_that_has_gone_is_neither_placed_nor_closed_again` and
    ///    `a_layout_placing_a_closed_window_does_not_show_it_again`.
    /// 4. [`Content::Leaving`] — the client has gone and the pane is still
    ///    being drawn, fading out: issue #126's state, which `Solium::depart`
    ///    enters. `gone` is set on every route into it, so this arm answers
    ///    nothing the third does not; it is here so that the predicate is total
    ///    over `Content` and cannot answer "not leaving" for a variant whose
    ///    name is `Leaving`.
    ///
    /// **Three places would keep a `Leaving` pane alive for ever, and this
    /// comment used to say so while nothing constructed one.** Each is
    /// answered now:
    ///
    /// * `Panes::sync` retains the panes whose `client()` is `None`, which is
    ///   how a loading pane survives a sweep; it drops one whose fade is over
    ///   (`syncing_retires_a_pane_that_has_finished_leaving`).
    /// * [`Self::expired`] answers for this state too, on [`LEAVING`], and
    ///   `Solium::settle_leaving` retires every pane it answers for, once a
    ///   frame, with nothing else having to happen
    ///   (`a_pane_that_has_left_expires_when_its_fade_is_over`,
    ///   `a_window_that_closes_itself_fades_out_and_is_gone_on_time`).
    /// * `Solium::close_pane` still declines it, through this function, which
    ///   is now the right answer rather than a trap: there is no client to ask
    ///   and it goes on its own (`a_window_that_left_is_nobodys_to_find`).
    pub(crate) const fn leaving(&self) -> bool {
        self.closing_at.is_some()
            || self.asked_at.is_some()
            || self.gone
            || matches!(self.content, Content::Leaving { .. })
    }

    /// The client's window, if one has arrived.
    pub(crate) const fn client(&self) -> Option<&Window> {
        match &self.content {
            Content::Client { window, .. } => Some(window),
            _ => None,
        }
    }
    pub(crate) const fn is_loading(&self) -> bool {
        matches!(self.content, Content::Loading { .. })
    }

    /// What this pane kept of a window whose client has gone, while it fades.
    /// `None` for every pane that is not [`Content::Leaving`].
    pub(crate) fn left(&self) -> Option<&Left> {
        match &self.content {
            Content::Leaving(left) => Some(left),
            _ => None,
        }
    }

    /// Whether this pane is the remains of a window whose client has gone.
    ///
    /// Such a pane is drawn and nothing else: it is in no window list, takes no
    /// input and no focus, and no layout places it. Every one of those is a
    /// filter somewhere asking this, and `a_window_that_left_is_nobodys_to_find`
    /// asks each of them.
    pub(crate) const fn ghost(&self) -> bool {
        matches!(self.content, Content::Leaving(_))
    }

    /// Take the scene that is standing on screen for this pane, if one is.
    ///
    /// A loading pane's scene, and a client's that has not painted yet -- the
    /// two cases where the scene is what anyone is looking at. Not a scene
    /// already dissolving off a client that has painted: there the client is
    /// what is on screen, and its picture is what a fade is drawn from.
    pub(crate) fn take_standing_scene(&mut self) -> Option<Box<crate::surface::ShellSurface>> {
        match &mut self.content {
            Content::Loading { scene, .. }
            | Content::Client {
                scene, faded: None, ..
            } => scene.take(),
            _ => None,
        }
    }

    /// Whether a scene is standing on screen for this pane: what
    /// [`Self::take_standing_scene`] would take.
    pub(crate) const fn has_standing_scene(&self) -> bool {
        matches!(
            self.content,
            Content::Loading { scene: Some(_), .. }
                | Content::Client {
                    scene: Some(_),
                    faded: None,
                    ..
                }
        )
    }

    /// Whether a client belonging to `family` — a process and its ancestors —
    /// is the one this pane is waiting for.
    pub(crate) fn awaits(&self, family: &[u32]) -> bool {
        match &self.content {
            Content::Loading { pid: Some(pid), .. } => family.contains(pid),
            _ => false,
        }
    }

    /// Give a pane the client it was waiting for.
    ///
    /// The id and the slot do not change, which is the whole point: nothing
    /// downstream learns that the content used to be something else, and a
    /// decoration keyed by pane id carries its animation straight through.
    pub(crate) fn adopt(&mut self, window: Window) {
        // The scene comes across with it. See `Content::Client::scene`: the
        // client has mapped and has not painted, and letting go of what is on
        // screen now would leave a hole for however long that takes.
        let scene = match &mut self.content {
            Content::Loading { scene, .. } => scene.take(),
            Content::Client { scene, .. } => scene.take(),
            Content::Leaving(_) => None,
        };
        self.content = Content::Client {
            window,
            scene,
            faded: None,
        };
        self.adopted = true;
    }

    /// The application has painted: begin letting go of the scene over it.
    ///
    /// Returns whether this call started the fade, so it is set up once.
    pub(crate) fn fade(&mut self, now: Duration) -> bool {
        match &mut self.content {
            Content::Client {
                scene: Some(_),
                faded: faded @ None,
                ..
            } => {
                *faded = Some(now);
                true
            }
            _ => false,
        }
    }

    /// Whether the scene has been fading for at least `over`.
    pub(crate) fn faded(&self, now: Duration, over: Duration) -> bool {
        match &self.content {
            Content::Client {
                faded: Some(began), ..
            } => now.saturating_sub(*began) >= over,
            _ => false,
        }
    }

    /// How much of the scene is still there.
    ///
    /// Solid until the application has painted, then away over `over`. Eased
    /// rather than linear, so it holds for a moment and then goes: a linear
    /// dissolve spends its whole length looking like a wash over the window,
    /// which reads as something being wrong with the window.
    pub(crate) fn scene_alpha(&self, now: Duration, over: Duration) -> f32 {
        let Content::Client {
            faded: Some(began), ..
        } = &self.content
        else {
            return 1.0;
        };
        if over.is_zero() {
            return 0.0;
        }
        let progress = now.saturating_sub(*began).as_secs_f64() / over.as_secs_f64();
        let gone = solium_animation::Curve::InOutQuad.at(progress.clamp(0.0, 1.0));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an alpha, clamped to 0..=1 before it is narrowed"
        )]
        let alpha = (1.0 - gone).clamp(0.0, 1.0) as f32;
        alpha
    }

    /// The application has painted: let go of the scene standing in for it.
    ///
    /// Returns whether there was one, so the handover can be reported once.
    pub(crate) fn filled(&mut self) -> bool {
        match &mut self.content {
            Content::Client { scene, .. } => scene.take().is_some(),
            _ => false,
        }
    }

    /// Whether anything of ours is drawing this pane.
    ///
    /// Including a pane whose client has gone and which kept the scene that
    /// was standing in for it: that pane is drawn by the same walk that draws
    /// a loading one, fading under its close.
    pub(crate) const fn has_scene(&self) -> bool {
        match &self.content {
            Content::Loading { scene, .. } | Content::Client { scene, .. } => scene.is_some(),
            Content::Leaving(left) => matches!(left.remains, Remains::Scene(_)),
        }
    }

    /// Whether a client mapped into this pane rather than creating it.
    pub(crate) const fn adopted(&self) -> bool {
        self.adopted
    }

    /// Whose process to wait for. Set after the fork, because the window went
    /// up before it — which is the point of the whole exercise.
    pub(crate) fn expect(&mut self, pid: u32) {
        if let Content::Loading { pid: waiting, .. } = &mut self.content {
            *waiting = Some(pid);
        }
    }

    /// The client has gone; keep what it left for the length of its fade.
    ///
    /// Whatever the pane held before goes: a client's `Window` handle, a
    /// loading pane's program. A scene worth keeping was taken out first, with
    /// [`Self::take_standing_scene`], and is in `left`.
    pub(crate) fn leave(&mut self, left: Left) {
        self.content = Content::Leaving(Box::new(left));
    }

    /// Whether this pane has waited out what it was waiting for.
    ///
    /// Two states wait. A loading pane waits for its application, and gives up
    /// after `patience`. A pane whose client has gone waits for its fade, and
    /// is done [`LEAVING`] after that fade began, whatever `patience` is -- it
    /// is measured by the close it is playing, not by how long an application
    /// may take to arrive. A pane with a client is the client's problem.
    pub(crate) fn expired(&self, now: Duration, patience: Duration) -> bool {
        match &self.content {
            Content::Loading { .. } => now.saturating_sub(self.opened) >= patience,
            Content::Leaving(left) => now.saturating_sub(left.since) >= LEAVING,
            Content::Client { .. } => false,
        }
    }

    /// Whether this pane's client has gone and its fade is over. What
    /// `Panes::sync` and `Solium::settle_leaving` retire a pane on.
    pub(crate) fn faded_out(&self, now: Duration) -> bool {
        self.ghost() && self.expired(now, Duration::MAX)
    }

    /// The scene drawing this pane, while it has no client to draw itself.
    pub(crate) fn scene_mut(&mut self) -> Option<&mut crate::surface::ShellSurface> {
        match &mut self.content {
            Content::Loading { scene, .. } | Content::Client { scene, .. } => scene.as_deref_mut(),
            Content::Leaving(left) => match &mut left.remains {
                Remains::Scene(scene) => Some(scene),
                Remains::Picture(_) | Remains::Lost => None,
            },
        }
    }

    /// How long this pane has been waiting for its application.
    pub(crate) fn waited(&self, now: Duration) -> Duration {
        now.saturating_sub(self.opened)
    }

    /// What to call it, before a client has an opinion.
    pub(crate) fn program(&self) -> Option<&str> {
        match &self.content {
            Content::Loading { program, .. } => Some(program),
            _ => None,
        }
    }
}

/// Every pane, bottom to top.
///
/// The same order `Space` stacks its elements in, because it is the order both
/// a hit test and a window list want — reversed, you are looking at what is on
/// top first, which is what "which window is this click for" means.
///
/// This is the compositor's own view. `Space` is still underneath and still the
/// authority on where a mapped client is and what damage it did; `sync` is the
/// one place the two are reconciled, so they cannot drift apart anywhere else.
#[derive(Debug, Default)]
pub(crate) struct Panes {
    panes: Vec<Pane>,
    /// Set when the list changed by some route other than `sync` — a pane
    /// opened, or one removed because its application never came.
    ///
    /// `sync` reports a change by comparing the list it starts with against
    /// the one it ends with, and everything keyed by a pane is tidied on that
    /// report. A change that happened before it started is invisible to that
    /// comparison, and a frame whose pane went that way would be kept forever.
    changed: bool,
}

impl Panes {
    /// Bottom to top. `.rev()` for a hit test.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &Pane> {
        self.panes.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.panes.len()
    }

    pub(crate) fn get(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|pane| pane.id == id)
    }

    /// The pane a script means by an id. Scripts hold the number rather than
    /// the type, because they got it through Lua.
    pub(crate) fn by_script_id(&self, id: u64) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id.get() == id)
    }

    /// The pane a client's window is the content of.
    pub(crate) fn of(&self, window: &Window) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.client() == Some(window))
    }

    pub(crate) fn id_of(&self, window: &Window) -> Option<PaneId> {
        self.of(window).map(Pane::id)
    }

    /// The pane waiting for a client from this process family, if any.
    ///
    /// Topmost first, so the most recently asked-for window wins when someone
    /// has asked for the same program twice.
    pub(crate) fn awaiting(&self, family: &[u32]) -> Option<PaneId> {
        self.panes
            .iter()
            .rev()
            .find(|pane| pane.awaits(family))
            .map(Pane::id)
    }

    /// Open a pane, on top. The window's life begins here.
    pub(crate) fn open(&mut self, pane: Pane) -> PaneId {
        let id = pane.id;
        self.panes.push(pane);
        self.changed = true;
        id
    }

    /// Forget a pane. Everything keyed by it goes at the next `sync`.
    pub(crate) fn remove(&mut self, id: PaneId) -> bool {
        let before = self.panes.len();
        self.panes.retain(|pane| pane.id != id);
        let removed = before != self.panes.len();
        self.changed |= removed;
        removed
    }

    /// Say the window list changed, for a change `sync` cannot see by itself.
    ///
    /// A pane whose client has just gone stays in the list, where it was, as
    /// what fades out -- so the ids `sync` compares are the ones it had, and it
    /// would report nothing. The pane has stopped being a window all the same,
    /// and a window going is what `sync_panes` settles the keyboard on:
    /// `a_window_that_left_is_nobodys_to_find` has the keyboard move.
    pub(crate) const fn changed(&mut self) {
        self.changed = true;
    }

    /// Take a pane for a client that arrived without being asked for, on top.
    ///
    /// Called where the window is mapped, so that nothing can observe a mapped
    /// window that has no pane. `sync` would create one at the next refresh
    /// anyway; this is what makes the id available *now*, to the script that is
    /// about to be told the window opened.
    pub(crate) fn mapped(
        &mut self,
        window: Window,
        slot: Rectangle<i32, Logical>,
        now: Duration,
    ) -> PaneId {
        if let Some(existing) = self.id_of(&window) {
            return existing;
        }
        let pane = Pane::mapped(window, slot, now);
        let id = pane.id;
        self.panes.push(pane);
        id
    }

    /// Reconcile against the space, given its elements bottom to top with the
    /// slot each one occupies.
    ///
    /// Three things happen here and nowhere else: a pane whose client has gone
    /// is retired, a client with no pane gets one, and the order is brought
    /// back in line with the stacking `Space` keeps. Doing it in one pass over
    /// one input is the only reason the two views can be trusted to agree —
    /// the alternative is remembering to do it at every site that maps or
    /// unmaps, which is the same discipline written down six times.
    ///
    /// Returns whether anything changed, so a caller can skip work that only
    /// matters when the window list is different.
    pub(crate) fn sync(
        &mut self,
        stack: &[(Window, Rectangle<i32, Logical>)],
        now: Duration,
    ) -> bool {
        let before: Vec<PaneId> = self.panes.iter().map(|pane| pane.id).collect();

        // Taken out one at a time and put back in the space's order. Whatever
        // is still here at the end has no client in the space.
        let mut held: Vec<Option<Pane>> = self.panes.drain(..).map(Some).collect();

        let mut ordered: Vec<Pane> = Vec::with_capacity(stack.len());
        for (window, slot) in stack {
            let mine = held
                .iter_mut()
                .find(|held| {
                    held.as_ref()
                        .is_some_and(|pane| pane.client() == Some(window))
                })
                .and_then(Option::take);
            // The space is the authority on where a mapped client is, and this
            // is the one place a pane is told so. A loading pane's slot is its
            // own, which is why the two cannot be the same assignment.
            let mut pane = mine.unwrap_or_else(|| Pane::mapped(window.clone(), *slot, now));
            // A window with no size has told us nothing: it has been mapped
            // and has not committed a buffer yet. Writing that over the slot
            // the layout gave the pane would throw the layout's answer away --
            // which is exactly the window between a client mapping into a pane
            // and its first commit.
            if slot.size.w > 0 && slot.size.h > 0 {
                pane.set_slot(*slot);
            }
            ordered.push(pane);
        }

        // A pane still waiting for a client stays, and rides on top, where a
        // window just asked for belongs.
        //
        // **So does one whose client has gone and which is fading out**
        // (`Content::Leaving`), until its fade is over, and then it is dropped
        // here if `Solium::settle_leaving` has not dropped it first. That is
        // step 5 of the window-provider migration: `Solium::depart` turns a
        // pane whose client went into one of these, drawn from what its client
        // left, and nothing else about it is a window any more. It does not
        // ride on top: it goes back where it was in the stack, directly above
        // the highest of the panes it is drawn over (`Left::over`, which says
        // why), and above any other that is fading out from the same place --
        // put there first, so below it before this sweep too.
        //
        // **A pane whose client went without `depart` hearing of it is still
        // dropped here, and vanishes**: its client is `Some`, and not in the
        // space. That is a client whose element smithay dropped in `refresh`
        // with no handler of ours called -- read, and not reached by a test:
        // every Wayland toplevel's destruction calls `toplevel_destroyed`, so
        // it would take an X11 window whose Xwayland went away without an
        // unmap. `syncing_retires_a_pane_that_has_finished_leaving` pins the
        // ghost half.
        for pane in held
            .into_iter()
            .flatten()
            .filter(|pane| pane.client().is_none() && !pane.faded_out(now))
        {
            let Some(left) = pane.left() else {
                ordered.push(pane);
                continue;
            };
            let above = ordered
                .iter()
                .rposition(|each| left.over.contains(&each.id))
                .map_or(0, |at| at + 1);
            let at = above
                + ordered
                    .iter()
                    .skip(above)
                    .take_while(|each| each.ghost())
                    .count();
            ordered.insert(at, pane);
        }
        self.panes = ordered;

        let after: Vec<PaneId> = self.panes.iter().map(|pane| pane.id).collect();
        std::mem::take(&mut self.changed) || before != after
    }
}

/// Which QML draws a pane that is still loading.
///
/// The same rule decorations follow, and for the same reason: a name picks one
/// of the scenes that ship, a name matching a file in the user's own directory
/// picks theirs instead, and a path picks anyone's. What a window looks like
/// while its application starts is a thing people will want to own.
///
/// ```sh
/// SOLIUM_LOADING=plain          # qml/loading/plain.qml
/// SOLIUM_LOADING=~/mine.qml     # anywhere
/// ```
///
/// A script's choice arrives through `chosen`, from the configuration; the
/// environment overrides it, because it is set per run by whoever started this
/// one.
pub(crate) fn loading_source(chosen: Option<&str>) -> PathBuf {
    let name = std::env::var("SOLIUM_LOADING")
        .ok()
        .or_else(|| chosen.map(ToOwned::to_owned));
    let Some(name) = name else {
        return shipped("window");
    };
    if name.contains('/') || name.ends_with(".qml") {
        return PathBuf::from(expand(&name));
    }
    if let Some(user) = crate::qml::user_qml_dir() {
        let theirs = user.join("loading").join(format!("{name}.qml"));
        if theirs.is_file() {
            return theirs;
        }
    }
    shipped(&name)
}

fn shipped(name: &str) -> PathBuf {
    crate::assets::qml()
        .join("loading")
        .join(format!("{name}.qml"))
}

/// Expand a leading `~`, since this comes from an environment variable and
/// nothing else will have done it.
fn expand(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => {
            std::env::var("HOME").map_or_else(|_| path.to_owned(), |home| format!("{home}/{rest}"))
        }
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> Rectangle<i32, Logical> {
        Rectangle::new((10, 20).into(), (300, 200).into())
    }

    /// What a pane keeps of a window that went at `since`, with nothing to
    /// draw it from.
    fn left(since: Duration) -> Left {
        Left {
            since,
            outer: slot(),
            geometry: slot(),
            groups: Vec::new(),
            over: Vec::new(),
            title: String::new(),
            focused: false,
            remains: Remains::Lost,
            fill: smithay::backend::renderer::element::Id::new(),
        }
    }

    #[test]
    fn identities_are_never_reused() {
        let first = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        let second = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn adoption_keeps_the_identity_and_the_slot() {
        // The property the whole design rests on: a client arriving must not
        // produce a *different* window. Nothing here can build a real
        // `Window`, so this asserts what can be asserted without one --
        // that adopting changes neither of the two things everything else
        // addresses a pane by.
        let mut pane = Pane::loading(
            "kitty",
            Some(42),
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
        let (id, where_it_was) = (pane.id(), pane.slot());
        assert!(pane.is_loading());
        assert!(pane.awaits(&[7, 42, 1]));

        pane.leave(left(Duration::from_millis(500)));
        assert_eq!(pane.id(), id);
        assert_eq!(pane.slot(), where_it_was);
        assert!(!pane.is_loading());
        assert!(!pane.awaits(&[42]), "a pane with content awaits nothing");
    }

    /// **What `show_if_new`'s placement guard (state.rs) relies on.** That
    /// function is the placement path for issue #100's XWayland half: an
    /// override-redirect window (a Steam context menu) arrives already
    /// placed by its own client, `take_unmanaged_pane` calls `unmanage` on
    /// its pane, and `show_if_new` must then never size or move it -- in
    /// either of its two branches -- on this commit or any later one.
    ///
    /// `show_if_new` itself cannot be exercised here. Like
    /// `adoption_keeps_the_identity_and_the_slot` above, nothing in this
    /// crate's tests can build a real `Window`, X11-backed or otherwise, so
    /// this pins the contract its guard depends on instead: `managed` must
    /// actually flip, and flipping it must be the *only* thing that
    /// happens. A test that stopped at `managed()` would pass even if
    /// `unmanage` also reset the slot to a default -- which would defeat the
    /// guard just as thoroughly as `show_if_new` never asking it to -- so
    /// the geometry is asserted too: the slot a pane already has when it
    /// becomes unmanaged is "the geometry it was mapped with" for a real
    /// override-redirect window, and it must survive untouched.
    #[test]
    fn an_unmanaged_pane_keeps_the_slot_it_was_mapped_with() {
        let mapped_with = Rectangle::new((640, 360).into(), (220, 140).into());
        let mut pane = Pane::loading(
            "steam-menu",
            None,
            mapped_with,
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
        assert!(pane.managed(), "a pane is managed until told otherwise");

        pane.unmanage();

        assert!(
            !pane.managed(),
            "unmanage must flip the exact flag show_if_new's guard and \
             snapshot's window-list filter both read"
        );
        assert_eq!(
            pane.slot(),
            mapped_with,
            "and must do nothing else -- the geometry a pane was mapped \
             with is what the placement guard exists to protect"
        );
    }

    #[test]
    fn a_loading_pane_gives_up_after_its_patience() {
        let patience = Duration::from_secs(8);
        let pane = Pane::loading(
            "slow",
            Some(9),
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
        assert!(!pane.expired(Duration::from_secs(7), patience));
        assert!(pane.expired(Duration::from_secs(8), patience));
    }

    /// **#126: a pane whose client has gone is done when its fade is, and
    /// patience has nothing to do with it.**
    ///
    /// This asserted the opposite until #126 -- that a leaving pane never
    /// expires at all -- which was one of the three things that would have
    /// kept the first `Content::Leaving` pane on screen for good.
    #[test]
    fn a_pane_that_has_left_expires_when_its_fade_is_over() {
        let patience = Duration::from_secs(8);
        let went = Duration::from_secs(1);
        let mut pane = Pane::loading(
            "slow",
            Some(9),
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
        pane.leave(left(went));
        assert!(pane.ghost(), "the premise: its client has gone");
        assert!(
            !pane.expired(went + LEAVING - Duration::from_millis(1), patience),
            "a pane still fading is not done"
        );
        assert!(
            pane.expired(went + LEAVING, Duration::MAX),
            "done when the fade is, however long patience is"
        );
        assert!(pane.faded_out(went + LEAVING));
        assert!(
            !pane.expired(went + LEAVING - Duration::from_millis(1), Duration::ZERO),
            "and not before it, however short patience is"
        );
    }

    /// **#126's review: `sync` keeps a pane fading out where it was in the
    /// stack**, directly above the highest pane it is drawn over, rather than
    /// on top with the panes still waiting for an application.
    ///
    /// Loading panes stand in for the windows here, because a `Window` cannot
    /// be made without a client; the rule is the same one for both, and
    /// `a_window_that_closes_itself_behind_another_fades_behind_it` drives it
    /// with real windows. Two panes fading out over the same one keep the order
    /// they were in.
    #[test]
    fn syncing_keeps_a_pane_that_left_where_it_was_in_the_stack() {
        let went = Duration::from_secs(3);
        let loading = |pid| {
            Pane::loading(
                "kitty",
                Some(pid),
                slot(),
                PathBuf::new(),
                None,
                Duration::ZERO,
            )
        };
        // On top of the list, where `Panes::open` puts a pane -- and where
        // #126 first put one whose client had gone, whatever it had been under.
        let mut panes = Panes::default();
        let bottom = panes.open(loading(1));
        let top = panes.open(loading(2));
        let first = panes.open(loading(3));
        let second = panes.open(loading(4));
        for going in [first, second] {
            if let Some(pane) = panes.get_mut(going) {
                pane.leave(Left {
                    over: vec![bottom],
                    ..left(went)
                });
            }
        }
        panes.sync(&[], went);
        let order: Vec<PaneId> = panes.iter().map(Pane::id).collect();
        assert_eq!(
            order,
            vec![bottom, first, second, top],
            "two panes fading out over the bottom one stay directly above it, in the \
             order they were in, and under the one that was over them"
        );
    }

    /// **#126: `sync` keeps a pane fading out and drops it when it is done.**
    ///
    /// `sync` keeps every pane with no client, which is how a loading pane
    /// survives a sweep, and a pane whose client has gone has none either --
    /// so without the second half of its filter the first ghost would have
    /// been kept for ever by the one function that reconciles the list.
    #[test]
    fn syncing_retires_a_pane_that_has_finished_leaving() {
        let went = Duration::from_secs(3);
        let mut panes = Panes::default();
        let staying = panes.open(Pane::loading(
            "kitty",
            Some(1),
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        ));
        let going = panes.open(Pane::loading(
            "kitty",
            Some(2),
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        ));
        if let Some(pane) = panes.get_mut(going) {
            pane.leave(left(went));
        }
        panes.sync(&[], went);

        assert!(
            !panes.sync(&[], went + LEAVING - Duration::from_millis(1)),
            "a pane still fading is kept, and nothing changed"
        );
        assert!(panes.get(going).is_some());
        assert!(
            panes.sync(&[], went + LEAVING),
            "dropping it is a change, and it is reported"
        );
        assert!(panes.get(going).is_none(), "done fading, and gone");
        assert!(
            panes.get(staying).is_some(),
            "and the pane still waiting for its application is not touched"
        );
    }

    #[test]
    fn the_loading_scene_is_a_setting() {
        // A bare name is one of ours; a path is anyone's. The environment is
        // checked first, so this asserts the configured path only when the
        // variable is unset -- tests share a process and an environment.
        if std::env::var_os("SOLIUM_LOADING").is_none() {
            assert!(
                loading_source(None).ends_with("qml/loading/window.qml"),
                "the default is the one that ships"
            );
            assert!(loading_source(Some("plain")).ends_with("qml/loading/plain.qml"));
            assert_eq!(
                loading_source(Some("/tmp/mine.qml")),
                PathBuf::from("/tmp/mine.qml")
            );
        }
    }

    #[test]
    fn syncing_against_an_empty_space_keeps_what_is_still_waiting() {
        // Nothing here can build a real `Window`, so what is testable is the
        // half that matters most anyway: a pane whose application has not
        // arrived must survive a reconcile that finds no client for it. Get
        // this wrong and a loading window is retired the frame after it
        // appears -- which looks exactly like the feature not working.
        let mut panes = Panes::default();
        let mut ids = Vec::new();
        for name in ["kitty", "firefox"] {
            let pane = Pane::loading(name, Some(1), slot(), PathBuf::new(), None, Duration::ZERO);
            ids.push(panes.open(pane));
        }

        assert!(
            panes.sync(&[], Duration::from_secs(1)),
            "opening them is a change, and it is reported once"
        );
        assert!(
            !panes.sync(&[], Duration::from_secs(2)),
            "and then nothing came and nothing went"
        );
        assert_eq!(panes.len(), 2);
        assert_eq!(
            panes.iter().map(Pane::id).collect::<Vec<_>>(),
            ids,
            "and they kept their order"
        );
    }

    #[test]
    fn a_script_addresses_a_pane_by_its_id() {
        let mut panes = Panes::default();
        let id = panes.open(Pane::loading(
            "kitty",
            None,
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        ));

        assert_eq!(panes.by_script_id(id.get()).map(Pane::id), Some(id));
        assert!(
            panes.by_script_id(id.get() + 1000).is_none(),
            "an id that no longer exists is not found, not a panic"
        );
    }

    #[test]
    fn a_pane_is_pending_until_it_is_told_otherwise() {
        // The default matters more than it looks: a pane in neither table is
        // one whose frame has not been built *yet*, and `insets_of` keeps
        // reserving room for it. A pane that defaulted to `None` would lose
        // that room and the window would jump when its frame arrived.
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        assert!(matches!(pane.frame(), Frame::Pending));
        pane.set_frame(Frame::None);
        assert!(matches!(pane.frame(), Frame::None));
    }

    #[test]
    fn a_pane_cannot_be_both_styled_and_bare() {
        // Not a runtime check. `Frame` is an enum and a pane holds exactly one,
        // so "styled *and* bare" cannot be written down — this test exists to
        // say that out loud, and to fail if a second source of truth ever comes
        // back.
        //
        // It could be written down before. `Decorations` held `frames:
        // HashMap<PaneId, Decoration>` and `bare: HashSet<PaneId>`, a pane
        // could be in both, and `Solium::insets_of` read `frames` first — so
        // such a pane was framed and its `bare` was silently ignored, and
        // nothing tested it.
        //
        // The match below is the assertion, and it is the part that would stop
        // compiling: it is exhaustive over `Frame`, so an arm meaning "framed,
        // but also bare" has to come here and be given an answer to the one
        // question everything else asks. What is checked at run time is that
        // both accessors derive their answer from that same single field.
        // `decoration.rs`'s `a_pane_cannot_be_framed_and_bare_at_once` takes
        // the same claim through `set_bare` — the mutator that used to leave a
        // frame standing — on a pane with a real `Decoration` on it.
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        for frame in [Frame::Pending, Frame::None] {
            pane.set_frame(frame);
            let styled = match pane.frame() {
                Frame::Styled(_) => true,
                Frame::Pending | Frame::None => false,
            };
            assert_eq!(
                pane.decoration().is_some(),
                styled,
                "whether there is a scene is the frame, not a second opinion \
                 about it"
            );
            assert_eq!(
                pane.decoration_mut().is_some(),
                styled,
                "and writing to it reaches the same value reading it does"
            );
        }

        // `Styled` is not among them because building one needs Qt, which this
        // module has no business starting. It is covered where a frame can
        // actually be built, in `decoration.rs`.
    }

    #[test]
    fn a_pane_has_no_timers_until_somebody_asks_it_to_close() {
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        assert!(pane.closing_at().is_none());
        assert!(pane.asked_at().is_none());

        // The sequence `close_pane` and `settle_closing` put a window through:
        // leave, then ask, then wait to see whether it went.
        pane.begin_closing(Duration::from_millis(190));
        assert_eq!(pane.closing_at(), Some(Duration::from_millis(190)));
        pane.stop_closing();
        pane.mark_asked(Duration::from_millis(190));
        assert!(pane.closing_at().is_none(), "the request has gone out");
        assert_eq!(
            pane.asked_at(),
            Some(Duration::from_millis(190)),
            "and it is still being waited on"
        );
        pane.forget_asked();
        assert!(pane.asked_at().is_none());
    }

    #[test]
    fn a_closing_timer_goes_when_the_pane_does() {
        // Why the timers moved in. They were `HashMap<PaneId, Duration>`
        // beside the panes, and an entry whose pane had gone stayed there
        // until `sync_panes` remembered to sweep it. There is no longer
        // anything to remember.
        let mut panes = Panes::default();
        let id = panes.open(Pane::loading(
            "kitty",
            None,
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        ));
        if let Some(pane) = panes.get_mut(id) {
            pane.begin_closing(Duration::from_millis(10));
            pane.mark_asked(Duration::from_millis(4));
        }
        assert_eq!(
            panes.get(id).and_then(Pane::closing_at),
            Some(Duration::from_millis(10))
        );

        assert!(panes.remove(id));
        assert!(
            panes.get(id).is_none(),
            "and both of its timers went with it; there is nothing left to retain"
        );
    }

    #[test]
    fn a_reconcile_does_not_forget_what_a_pane_was_told() {
        // `sync` drains the list and rebuilds it. A timer that did not ride
        // along would be a window told to close and then never asked -- which
        // is the failure the old `retain` lines could not have caused and a
        // move like this one can.
        let mut panes = Panes::default();
        let id = panes.open(Pane::loading(
            "kitty",
            None,
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        ));
        if let Some(pane) = panes.get_mut(id) {
            pane.begin_closing(Duration::from_millis(10));
            pane.mark_asked(Duration::from_millis(4));
        }

        panes.sync(&[], Duration::from_secs(1));
        assert_eq!(
            panes.get(id).and_then(Pane::closing_at),
            Some(Duration::from_millis(10))
        );
        assert_eq!(
            panes.get(id).and_then(Pane::asked_at),
            Some(Duration::from_millis(4))
        );
    }

    #[test]
    fn a_pane_with_no_process_adopts_nothing() {
        let pane = Pane::loading(
            "mystery",
            None,
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
        assert!(!pane.awaits(&[1, 2, 3]));
    }

    /// **A `Frame` costs what the decoration inside it costs, and not a byte
    /// more.**
    ///
    /// The claim the `#[expect(clippy::large_enum_variant)]` on `Frame` used to
    /// carry in prose, asserted instead — because that comment's number went
    /// stale the moment a `Decoration` changed shape, and a suppression with a
    /// false number attached is worse than one with no reason at all.
    ///
    /// Written as a relation rather than as two literals on purpose. The
    /// numbers move whenever anything in a decoration moves — they already have
    /// once, 104 to 112, when bleed put the pane's size into `Shown` — and the
    /// fact worth keeping is not what they are but that they are *equal*: the
    /// discriminant lands in a niche inside the payload, so the three-armed
    /// enum is free relative to the one arm that carries anything. Lose the
    /// niche and this fails, which is exactly when boxing would be worth
    /// re-arguing.
    #[test]
    fn a_frame_costs_what_the_decoration_in_it_costs() {
        assert_eq!(
            size_of::<Frame>(),
            size_of::<crate::decoration::Decoration>(),
            "`Frame::Styled` stopped fitting its discriminant into a niche, so \
             a pane now pays for the tag as well as for the decoration -- and \
             the case for boxing, which was rejected on these two numbers being \
             equal, is worth making again"
        );
    }

    /// **#133: the tile a window left, kept for its way back and no longer.**
    ///
    /// Four promises the three methods make between them, each asserted on the
    /// one pane in the order a session would reach them:
    ///
    ///  1. Leaving a tile keeps it, and the way back puts it back.
    ///  2. A window maximised and then sent fullscreen left its tile at the
    ///     maximise, and the fullscreen -- a second `leave_tile` with no tile to
    ///     take -- does not forget it.
    ///  3. A tile a layout has placed the window in since wins over the kept
    ///     one.
    ///  4. A window the layout lets go of while it is maximised is not put back
    ///     in its old tile by the un-maximise that follows.
    #[test]
    fn a_tile_left_for_a_maximise_is_kept_for_the_way_back_and_no_longer() {
        let tile = Rectangle::new((0, 0).into(), (494, 600).into());
        let newer = Rectangle::new((506, 0).into(), (494, 600).into());
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);

        pane.set_placed(tile);
        pane.leave_tile();
        assert_eq!(pane.placed(), None, "a maximised window is in no tile");
        pane.leave_tile();
        pane.return_to_tile();
        assert_eq!(
            pane.placed(),
            Some(tile),
            "and the way back puts it in the tile it left, through a second \
             leave with nothing to take"
        );

        pane.leave_tile();
        pane.set_placed(newer);
        pane.return_to_tile();
        assert_eq!(
            pane.placed(),
            Some(newer),
            "a layout's newer tile is not overwritten by the kept one"
        );

        pane.leave_tile();
        pane.untile();
        pane.return_to_tile();
        assert_eq!(
            pane.placed(),
            None,
            "a window let go by its layout stays out of every tile"
        );
    }

    /// **#128 with #133: a let-go that arrives while a window is leaving waits
    /// until the window is back.**
    ///
    /// Each promise [`Pane::let_go`] makes, on one pane, in the order a session
    /// would reach them. The pane holds both tiles -- one it left for a
    /// maximise, and one a sweep has put it in since -- so that "both" can be
    /// seen to mean both:
    ///
    ///  1. Let go while it is leaving, a window keeps its tiles.
    ///  2. A rescue moves the tile it is in and leaves the let-go owed.
    ///  3. Coming back takes the let-go, out of both tiles.
    ///  4. A take that is put back, as a declined give-back does, is owed
    ///     again with both tiles.
    ///  5. A tile given since cancels it.
    ///  6. A window that is not leaving is let go at once, as it always was.
    #[test]
    fn a_let_go_while_leaving_waits_until_the_window_is_back() {
        let left = Rectangle::new((0, 0).into(), (494, 600).into());
        let tile = Rectangle::new((506, 0).into(), (494, 600).into());
        let rescued = Rectangle::new((1426, 0).into(), (494, 600).into());
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        pane.set_placed(left);
        pane.leave_tile();
        pane.set_placed(tile);
        let tiles = |pane: &Pane| (pane.placed(), pane.left_tile());

        pane.begin_closing(Duration::from_millis(190));
        assert!(pane.leaving(), "the premise: the pane is being closed");
        pane.untile();
        assert_eq!(
            tiles(&pane),
            (Some(tile), Some(left)),
            "a window let go while it is leaving keeps its tiles"
        );

        pane.move_tile(rescued);
        assert_eq!(
            tiles(&pane),
            (Some(rescued), Some(left)),
            "a rescue moves the tile the window is in"
        );
        let held = pane.take_let_go();
        assert_eq!(
            tiles(&pane),
            (None, None),
            "and leaves the let-go owed, so coming back takes the window out of both"
        );

        pane.owe_let_go(held.expect("a let-go was owed"));
        assert_eq!(
            tiles(&pane),
            (Some(rescued), Some(left)),
            "a take that is put back gives back both tiles"
        );
        pane.set_placed(tile);
        assert_eq!(
            pane.take_let_go(),
            None,
            "a tile given since is the layout's newer word, and cancels the let-go"
        );
        assert_eq!(tiles(&pane), (Some(tile), Some(left)));

        pane.stop_closing();
        assert!(!pane.leaving(), "the premise: the pane is back");
        pane.untile();
        assert_eq!(
            tiles(&pane),
            (None, None),
            "a window that is not leaving is let go at once"
        );
    }

    /// **Issue #92: a window's way back is not its frame's to lose.**
    ///
    /// The rect a window goes back to used to live on the `Decoration`, so
    /// the `remove` that drops a window's frame as it goes fullscreen dropped
    /// the rect too. This sets a rect and then changes the frame under it
    /// three times -- `None`, `Pending`, `None`, starting from a loading pane
    /// -- to show that no `set_frame` touches it.
    ///
    /// Not the fullscreen sequence itself, which starts and ends on `Styled`:
    /// `Styled`, `Pending` (`remove`), `None` (`set_bare`), `Pending`
    /// (`unset_bare`), `Styled` (`insert`). Building a `Styled` frame needs Qt,
    /// which this module does not start, so `decoration.rs`'s
    /// `a_rebuilt_frame_does_not_take_the_way_back_with_it` walks that order
    /// on a pane with a real `Decoration` on it.
    #[test]
    fn a_windows_way_back_outlives_its_frame() {
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        assert_eq!(
            pane.restore(),
            None,
            "a new window has nowhere to go back to"
        );

        let before = Rectangle::new((400, 300).into(), (640, 480).into());
        pane.set_restore(Some(before));
        pane.set_frame(Frame::None);
        pane.set_frame(Frame::Pending);
        pane.set_frame(Frame::None);

        assert_eq!(
            pane.restore(),
            Some(before),
            "reading it does not use it up"
        );
        assert_eq!(
            pane.take_restore(),
            Some(before),
            "the frame came and went three times, and the way back is still here"
        );
        assert_eq!(
            pane.take_restore(),
            None,
            "and it is used once: the next maximise toggle maximises, rather \
             than jumping back to a rect from before the last one"
        );
    }
}
