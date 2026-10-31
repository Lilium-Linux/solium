//! An X11 window's pointer, drawn as the shape it names (#364).
//!
//! XWayland is one Wayland client standing in for every X11 one, and it hands
//! the compositor each X11 cursor the only way `wl_pointer` can: as a cursor
//! surface with the X client's pixels in it. Drawn as it is, that is the X
//! client's picture rather than the configured pointer, so the pointer changed
//! look as it crossed into Steam and every other X11 window.
//!
//! **What the pixels do not say, the X server does.** A cursor an X client
//! loads from a cursor theme by name -- libXcursor's `XcursorLibraryLoadCursor`,
//! and `XCreateFontCursor` once a theme is in use -- carries that name, set
//! with XFixes' `SetCursorName`, and XFixes tells any client that asks
//! (`SelectCursorInput` with `DisplayCursorNotify`) each time the displayed
//! cursor changes, with the new cursor's name. So a second connection to
//! XWayland, beside smithay's window manager, listens for that, [`icon`] turns
//! a name into the shape it stands for, and `Pointer::showing` draws an
//! XWayland cursor surface as that shape while the X server says the cursor
//! has a name we know.
//!
//! **A cursor with no name is known by its picture (#442).** Chromium, and so
//! Steam's window, which is Chromium, reads cursor theme files itself and
//! builds each cursor from the pixels, with no name: over Steam the pointer
//! was the theme GTK's settings name (Breeze, from KDE) in the middle of the
//! window and Solium's own at its frame. So the connection also reads the
//! cursor themes an X client could have loaded from -- Solium's own, if one
//! is configured, `XCURSOR_THEME`, the X resources' `Xcursor.theme`, GTK's
//! `gtk-cursor-theme-name`, and `default` -- and keeps every picture in them
//! by its size, hotspot and pixels. A cursor X reports with no name is asked
//! for its picture (`GetCursorImage`), and one that is a theme's picture of a
//! standard cursor is drawn as that shape. That is why this is done here,
//! rather than by exporting a theme to X clients: Solium's pointer may be a
//! QML scene, which is no theme an X client can load, and a client that reads
//! its theme from GTK's settings never looks at an exported one.
//!
//! **A cursor with no name we know, and no picture we know, stays the X
//! client's picture.** A game drawing its own crosshair or sword builds it from pixels
//! and names it nothing, and it must be left alone. The same holds for a
//! hidden pointer: XWayland hides it with `wl_pointer.set_cursor` and no
//! surface, which is `Hidden` and is never touched here, and a client that
//! hides it with a blank cursor of its own has a picture with no name.
//!
//! **An animated cursor stays the X client's picture too**, and not by
//! choice: the X server shows it one frame at a time, each frame a cursor of
//! its own with no name, and reports each one; the name is on the animation,
//! which it never reports. So `watch` and `left_ptr_watch` in a theme that
//! animates them are drawn by the X client, frame by frame.
//!
//! **No round trip per motion.** The X server sends the name once per change
//! of cursor, the connection lives on a thread of its own and blocks there,
//! and the compositor only hears a name when it is not the last one it heard.
//!
//! The one cursor of the compositor's own on the X side is the root window's,
//! which every X11 window that sets none shows. smithay sets it from the core
//! cursor font with no name, so this connection replaces it with the same
//! arrow named `left_ptr`: an X11 window that never asked for a cursor gets
//! the configured arrow, as a Wayland window does, rather than the X server's
//! black-and-white one.

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use smithay::{
    input::pointer::CursorIcon,
    reexports::{
        calloop::{
            LoopHandle,
            channel::{self, Sender},
        },
        wayland_server::{Resource as _, backend::ClientId, protocol::wl_surface::WlSurface},
    },
};
use x11rb::{
    connection::Connection as _,
    protocol::{
        Event,
        xfixes::{ConnectionExt as _, CursorNotifyMask},
        xproto::{Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, Window},
    },
};

use crate::xwayland::HasSolium;

/// `left_ptr` in the core cursor font: its glyph, and its mask is the next
/// one. What smithay sets on the root window, and what [`name_the_root`] sets
/// again with a name.
const LEFT_PTR: u16 = 68;

/// The shape a standard X cursor name stands for, or `None` for a name that
/// is not one.
///
/// The names are the ones cursor themes have files under and toolkits load
/// cursors by: the CSS names a theme written this decade ships, the X11 core
/// cursor font's (`left_ptr`, `xterm`, `hand2`, `sb_h_double_arrow`), and the
/// spellings Qt and GTK themes add (`size_hor`, `openhand`). It is
/// `shape::names` read the other way, with one meaning chosen where that list
/// files a name under two shapes as a fallback:
///
/// - `hand1` is the pointing hand, as the core font draws it, not `grab`.
/// - `fleur` is the four-way arrow, `move`, not a hand or `all-scroll`.
/// - `copy` is `copy`, not the drag-and-drop `dnd-ask` that borrows it.
/// - `circle` and `forbidden` are `not-allowed`, the meaning a toolkit loads
///   them for; `no-drop` has names of its own.
/// - `sb_h_double_arrow` and `sb_v_double_arrow` are the plain resizes, not
///   the column and row dividers, which have `split_h` and `split_v`.
///
/// Exact and case-sensitive, as X names are. `tests::every_shape_is_known_by_its_own_css_name`
/// and `tests::every_x11_spelling_a_theme_lists_is_known`.
pub(crate) fn icon(name: &str) -> Option<CursorIcon> {
    Some(match name {
        "default" | "left_ptr" | "arrow" | "top_left_arrow" | "left_arrow" => CursorIcon::Default,
        "context-menu" | "context_menu" => CursorIcon::ContextMenu,
        "help" | "question_arrow" | "whats_this" | "left_ptr_help" => CursorIcon::Help,
        "pointer" | "hand2" | "hand1" | "hand" | "pointing_hand" => CursorIcon::Pointer,
        "progress" | "left_ptr_watch" | "half-busy" => CursorIcon::Progress,
        "wait" | "watch" => CursorIcon::Wait,
        "cell" | "plus" => CursorIcon::Cell,
        "crosshair" | "cross" | "tcross" | "cross_reverse" | "diamond_cross" => {
            CursorIcon::Crosshair
        }
        "text" | "xterm" | "ibeam" => CursorIcon::Text,
        "vertical-text" | "vertical_text" => CursorIcon::VerticalText,
        "alias" | "link" | "dnd-link" => CursorIcon::Alias,
        "copy" | "dnd-copy" => CursorIcon::Copy,
        "move" | "dnd-move" | "fleur" => CursorIcon::Move,
        "no-drop" | "dnd-no-drop" => CursorIcon::NoDrop,
        "not-allowed" | "crossed_circle" | "forbidden" | "circle" => CursorIcon::NotAllowed,
        "grab" | "openhand" => CursorIcon::Grab,
        "grabbing" | "closedhand" | "dnd-none" => CursorIcon::Grabbing,
        "e-resize" | "right_side" => CursorIcon::EResize,
        "n-resize" | "top_side" => CursorIcon::NResize,
        "ne-resize" | "top_right_corner" => CursorIcon::NeResize,
        "nw-resize" | "top_left_corner" => CursorIcon::NwResize,
        "s-resize" | "bottom_side" => CursorIcon::SResize,
        "se-resize" | "bottom_right_corner" => CursorIcon::SeResize,
        "sw-resize" | "bottom_left_corner" => CursorIcon::SwResize,
        "w-resize" | "left_side" => CursorIcon::WResize,
        "ew-resize" | "h_double_arrow" | "sb_h_double_arrow" | "size_hor" => CursorIcon::EwResize,
        "ns-resize" | "v_double_arrow" | "sb_v_double_arrow" | "size_ver" => CursorIcon::NsResize,
        "nesw-resize" | "fd_double_arrow" | "size_bdiag" => CursorIcon::NeswResize,
        "nwse-resize" | "bd_double_arrow" | "size_fdiag" => CursorIcon::NwseResize,
        "col-resize" | "split_h" => CursorIcon::ColResize,
        "row-resize" | "split_v" => CursorIcon::RowResize,
        "all-scroll" | "size_all" => CursorIcon::AllScroll,
        "zoom-in" | "zoom_in" => CursorIcon::ZoomIn,
        "zoom-out" | "zoom_out" => CursorIcon::ZoomOut,
        "dnd-ask" => CursorIcon::DndAsk,
        "all-resize" => CursorIcon::AllResize,
        _ => return None,
    })
}

/// Whether a cursor surface stands for a named shape: only XWayland's, and
/// only while the X cursor has a name [`icon`] knows.
///
/// `owner` is the client the cursor surface belongs to and `xwayland` is
/// XWayland's, both `None` when there is none: a surface whose client has gone
/// and a session with no XWayland are not the same client, so two `None`s
/// never match. Generic over the client so the rule is tested without a
/// display; `tests::only_xwaylands_cursor_surface_stands_for_a_shape`.
pub(crate) fn stands_for<C: PartialEq>(
    owner: Option<&C>,
    xwayland: Option<&C>,
    named: Option<CursorIcon>,
) -> Option<CursorIcon> {
    match (owner, xwayland) {
        (Some(owner), Some(xwayland)) if owner == xwayland => named,
        _ => None,
    }
}

/// What the X side has said about the pointer, kept beside it in
/// `cursor::Pointer`.
#[derive(Debug, Default)]
pub(crate) struct Named {
    /// XWayland's client, once it is up.
    xwayland: Option<ClientId>,
    /// The shape the X cursor's name stands for, or `None` while it has no
    /// name [`icon`] knows.
    icon: Option<CursorIcon>,
}

impl Named {
    /// XWayland is up, as this client.
    pub(crate) fn xwayland(&mut self, client: ClientId) {
        self.xwayland = Some(client);
    }

    /// The X cursor now stands for `icon`. Returns whether that changed.
    pub(crate) fn name(&mut self, icon: Option<CursorIcon>) -> bool {
        if self.icon == icon {
            return false;
        }
        self.icon = icon;
        true
    }

    /// The shape `surface`, a client's cursor surface, is drawn as, or `None`
    /// to draw its own pixels. See [`stands_for`].
    pub(crate) fn of(&self, surface: &WlSurface) -> Option<CursorIcon> {
        // Not asked for when there is no name, which is every Wayland
        // client's surface in a session whose X cursor has none.
        self.icon?;
        let owner = surface.client().map(|client| client.id());
        stands_for(owner.as_ref(), self.xwayland.as_ref(), self.icon)
    }
}

/// Listen to XWayland, on display `display`, for the name of each cursor it
/// shows, and hand each to the pointer.
///
/// Called once XWayland is ready and smithay is its window manager. A failure
/// is said once and leaves X11 windows with their own pixels, which is how
/// they were drawn before this existed.
pub(crate) fn watch<D>(handle: &LoopHandle<'static, D>, display: u32, theme: Option<String>)
where
    D: HasSolium + 'static,
{
    let (sender, names) = channel::channel::<Option<CursorIcon>>();
    let inserted = handle.insert_source(names, |event, (), data| {
        if let channel::Event::Msg(shape) = event {
            let solium = data.solium();
            if solium.pointer.x11(shape) {
                tracing::debug!(?shape, "the X11 cursor changed");
                solium.redraw = true;
            }
        }
    });
    if let Err(err) = inserted {
        tracing::warn!(
            ?err,
            "could not hear X11 cursor names; X11 windows draw their own"
        );
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("solium-x11-cursor".to_owned())
        .spawn(move || {
            if let Err(err) = listen(display, theme.as_deref(), &sender) {
                tracing::warn!(
                    ?err,
                    "stopped hearing X11 cursor names; X11 windows draw their own"
                );
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(
            ?err,
            "no thread to hear X11 cursor names; X11 windows draw their own"
        );
    }
}

/// The connection's whole life: name the root's cursor, learn the themes'
/// pictures, then send the shape of every cursor the X server shows until
/// XWayland or the compositor goes.
fn listen(display: u32, theme: Option<&str>, shapes: &Sender<Option<CursorIcon>>) -> Result<()> {
    let (conn, screen) = x11rb::connect(Some(&format!(":{display}")))?;
    let root = conn
        .setup()
        .roots
        .get(screen)
        .map(|screen| screen.root)
        .context("XWayland has no screen")?;
    // XFixes must be told which version a client speaks before it answers
    // anything; cursor names arrived in version 2.
    conn.xfixes_query_version(5, 0)?.reply()?;
    conn.xfixes_select_cursor_input(root, CursorNotifyMask::DISPLAY_CURSOR)?;
    name_the_root(&conn, root)?;
    let environment = std::env::var("XCURSOR_THEME").ok();
    let themes = themes(
        theme,
        environment.as_deref(),
        resources(&conn, root).as_deref(),
        gtk_settings_theme().as_deref(),
    );
    let pictures = Pictures::load(&themes);
    tracing::info!(
        ?themes,
        pictures = pictures.len(),
        "X11 cursors are known by name and by these themes' pictures"
    );
    // Asked after selecting the events, so a change in between is heard
    // rather than lost.
    let showing = conn.xfixes_get_cursor_image_and_name()?.reply()?;
    let first = if showing.cursor_atom == x11rb::NONE {
        pictures.shape(&Picture::of(
            (showing.width, showing.height),
            (showing.xhot, showing.yhot),
            &showing.cursor_image,
        ))
    } else {
        icon(&String::from_utf8_lossy(&showing.name))
    };
    if shapes.send(first).is_err() {
        return Ok(());
    }
    let mut last = showing.cursor_atom;
    // Atoms are never freed by an X server, so a name, once looked up, is
    // that atom's for the life of the connection.
    let mut known: HashMap<Atom, String> = HashMap::new();
    loop {
        let event = match conn.wait_for_event() {
            Ok(event) => event,
            // XWayland has gone, which at the end of a session is every time,
            // and smithay says so when it is not.
            Err(err) => {
                tracing::debug!(
                    ?err,
                    "XWayland closed the connection that hears cursor names"
                );
                return Ok(());
            }
        };
        let Event::XfixesCursorNotify(notify) = event else {
            continue;
        };
        let shape = if notify.name == x11rb::NONE {
            // No name: every unnamed cursor is a different picture, so each
            // is asked for, the next frame of an animated one included.
            last = x11rb::NONE;
            let image = conn.xfixes_get_cursor_image()?.reply()?;
            pictures.shape(&Picture::of(
                (image.width, image.height),
                (image.xhot, image.yhot),
                &image.cursor_image,
            ))
        } else {
            // Another cursor by the same name: nothing to say.
            if notify.name == last {
                continue;
            }
            last = notify.name;
            let name = if let Some(name) = known.get(&notify.name) {
                name.clone()
            } else {
                let reply = conn.get_atom_name(notify.name)?.reply()?;
                let name = String::from_utf8_lossy(&reply.name).into_owned();
                known.insert(notify.name, name.clone());
                name
            };
            icon(&name)
        };
        if shapes.send(shape).is_err() {
            // The compositor has stopped listening, which is it exiting.
            return Ok(());
        }
    }
}

/// A cursor's picture as X reports it, `GetCursorImage`'s fields, reduced to
/// what tells two pictures apart: the size, the hotspot and a hash of the
/// pixels, premultiplied ARGB words as both the X server and a theme file
/// keep them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Picture {
    size: (u16, u16),
    hotspot: (u16, u16),
    pixels: u64,
}

impl Picture {
    pub(crate) fn of(size: (u16, u16), hotspot: (u16, u16), pixels: &[u32]) -> Self {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        pixels.hash(&mut hasher);
        Self {
            size,
            hotspot,
            pixels: hasher.finish(),
        }
    }

    /// A theme file's image, or `None` for one too large for X's sizes or
    /// carrying fewer pixels than it claims.
    fn of_file(image: &xcursor::parser::Image) -> Option<Self> {
        let size = (
            u16::try_from(image.width).ok()?,
            u16::try_from(image.height).ok()?,
        );
        let hotspot = (
            u16::try_from(image.xhot).ok()?,
            u16::try_from(image.yhot).ok()?,
        );
        let count = usize::from(size.0) * usize::from(size.1);
        // Little-endian ARGB words: see `theme::Ready::from_image`.
        let pixels: Vec<u32> = image
            .pixels_rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        (pixels.len() == count).then(|| Self::of(size, hotspot, &pixels))
    }
}

/// The pictures of every standard cursor in some themes, each by the shape
/// it stands for. The first theme to draw a picture names its shape.
#[derive(Debug, Default)]
pub(crate) struct Pictures(HashMap<Picture, CursorIcon>);

impl Pictures {
    /// Every image, every size and every frame, of every name [`icon`]
    /// knows, in `themes`, in order. A theme that is not installed adds
    /// nothing; a file two names share is read once.
    fn load(themes: &[String]) -> Self {
        let mut pictures = Self::default();
        let mut read = std::collections::HashSet::new();
        for name in themes {
            let theme = xcursor::CursorTheme::load(name);
            for shape in crate::cursor::shape::EVERY {
                for cursor in crate::cursor::shape::names(shape) {
                    let (Some(meant), Some(path)) = (icon(cursor), theme.load_icon(cursor)) else {
                        continue;
                    };
                    let path = std::fs::canonicalize(&path).unwrap_or(path);
                    if !read.insert(path.clone()) {
                        continue;
                    }
                    let Some(images) = std::fs::read(&path)
                        .ok()
                        .and_then(|bytes| xcursor::parser::parse_xcursor(&bytes))
                    else {
                        continue;
                    };
                    for image in &images {
                        if let Some(picture) = Picture::of_file(image) {
                            pictures.learn(picture, meant);
                        }
                    }
                }
            }
        }
        pictures
    }

    fn learn(&mut self, picture: Picture, shape: CursorIcon) {
        self.0.entry(picture).or_insert(shape);
    }

    /// The shape a picture is, if it is one a theme draws.
    pub(crate) fn shape(&self, picture: &Picture) -> Option<CursorIcon> {
        self.0.get(picture).copied()
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

/// The cursor theme names an X client may have loaded its cursors from,
/// first to last, each once: Solium's own (`configured`), `XCURSOR_THEME`,
/// the X resources' `Xcursor.theme` (`resources`, the root window's
/// `RESOURCE_MANAGER`), GTK's, and `default`, which is what libXcursor and
/// Chromium load with no theme named.
/// `tests::the_themes_are_solium_s_then_the_environment_s_then_the_resources_then_gtk_s`.
pub(crate) fn themes(
    configured: Option<&str>,
    environment: Option<&str>,
    resources: Option<&str>,
    gtk: Option<&str>,
) -> Vec<String> {
    let mut themes: Vec<String> = Vec::new();
    for theme in [
        configured.map(str::to_owned),
        environment.map(str::to_owned),
        resources.and_then(resource_theme),
        gtk.map(str::to_owned),
        Some("default".to_owned()),
    ]
    .into_iter()
    .flatten()
    {
        let theme = theme.trim().to_owned();
        if !theme.is_empty() && !themes.contains(&theme) {
            themes.push(theme);
        }
    }
    themes
}

/// `Xcursor.theme` in an X resources string, as `xrdb -query` prints it.
pub(crate) fn resource_theme(resources: &str) -> Option<String> {
    resources.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "Xcursor.theme").then(|| value.trim().to_owned())
    })
}

/// The root window's `RESOURCE_MANAGER`, the X resources `xrdb` loaded, if
/// any were.
fn resources(conn: &impl x11rb::connection::Connection, root: Window) -> Option<String> {
    let reply = conn
        .get_property(
            false,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            0,
            u32::MAX / 4,
        )
        .ok()?
        .reply()
        .ok()?;
    (!reply.value.is_empty()).then(|| String::from_utf8_lossy(&reply.value).into_owned())
}

/// The cursor theme GTK's own settings name, which GTK, and Chromium beside
/// it, load X cursors from when nothing else names one:
/// `gtk-cursor-theme-name` in GTK 3's or 4's `settings.ini`, or in
/// `~/.gtkrc-2.0`, the first that has one.
fn gtk_settings_theme() -> Option<String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    [
        config.as_ref().map(|dir| dir.join("gtk-3.0/settings.ini")),
        config.as_ref().map(|dir| dir.join("gtk-4.0/settings.ini")),
        home.as_ref().map(|dir| dir.join(".gtkrc-2.0")),
    ]
    .into_iter()
    .flatten()
    .filter_map(|file| std::fs::read_to_string(file).ok())
    .find_map(|text| gtk_theme(&text))
}

/// `gtk-cursor-theme-name` in a GTK settings file, quoted (`~/.gtkrc-2.0`)
/// or not (`settings.ini`).
pub(crate) fn gtk_theme(settings: &str) -> Option<String> {
    settings.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "gtk-cursor-theme-name")
            .then(|| value.trim().trim_matches('"').to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// Give the root window smithay's arrow again, named `left_ptr`.
///
/// The root's cursor is what an X11 window that sets none shows, and smithay
/// builds it from the core cursor font with no name. Named, it is drawn as the
/// configured arrow; see the module header.
fn name_the_root(conn: &impl x11rb::connection::Connection, root: Window) -> Result<()> {
    let font = conn.generate_id()?;
    conn.open_font(font, b"cursor")?;
    let cursor = conn.generate_id()?;
    conn.create_glyph_cursor(
        cursor,
        font,
        font,
        LEFT_PTR,
        LEFT_PTR + 1,
        0,
        0,
        0,
        u16::MAX,
        u16::MAX,
        u16::MAX,
    )?;
    conn.xfixes_set_cursor_name(cursor, b"left_ptr")?;
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().cursor(cursor))?;
    // The root holds its own reference to the cursor, so neither is needed
    // past this.
    conn.free_cursor(cursor)?;
    conn.close_font(font)?;
    conn.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use smithay::input::pointer::CursorIcon;

    use super::{Picture, Pictures, gtk_theme, icon, resource_theme, stands_for, themes};
    use crate::cursor::shape::{self, tests::EVERY};

    /// Every shape is known by its own CSS name, which is what a theme written
    /// this decade names its files and what a toolkit that loads a cursor by
    /// its CSS name asks for.
    #[test]
    fn every_shape_is_known_by_its_own_css_name() {
        for shape in EVERY {
            assert_eq!(icon(shape.name()), Some(shape), "{}", shape.name());
        }
    }

    /// Every spelling `shape::names` lists for a theme is known, so no name a
    /// theme has a file under is drawn as the X client's pixels for want of
    /// an entry here. Where it lists a name under two shapes as a fallback,
    /// the shape it means is the one asserted below.
    #[test]
    fn every_x11_spelling_a_theme_lists_is_known() {
        for shape in EVERY {
            for name in shape::names(shape) {
                assert!(icon(name).is_some(), "{name}, listed for {}", shape.name());
            }
        }
    }

    /// The names the issue names, and the ones `shape::names` lists under two
    /// shapes, as the shape each one means.
    #[test]
    fn the_x11_names_are_the_shapes_they_mean() {
        for (name, shape) in [
            ("left_ptr", CursorIcon::Default),
            ("xterm", CursorIcon::Text),
            ("hand2", CursorIcon::Pointer),
            ("hand1", CursorIcon::Pointer),
            ("watch", CursorIcon::Wait),
            ("left_ptr_watch", CursorIcon::Progress),
            ("question_arrow", CursorIcon::Help),
            ("crosshair", CursorIcon::Crosshair),
            ("fleur", CursorIcon::Move),
            ("sb_h_double_arrow", CursorIcon::EwResize),
            ("sb_v_double_arrow", CursorIcon::NsResize),
            ("top_left_corner", CursorIcon::NwResize),
            ("bottom_right_corner", CursorIcon::SeResize),
            ("fd_double_arrow", CursorIcon::NeswResize),
            ("bd_double_arrow", CursorIcon::NwseResize),
            ("split_h", CursorIcon::ColResize),
            ("copy", CursorIcon::Copy),
            ("circle", CursorIcon::NotAllowed),
            ("forbidden", CursorIcon::NotAllowed),
            ("openhand", CursorIcon::Grab),
            ("closedhand", CursorIcon::Grabbing),
            ("size_all", CursorIcon::AllScroll),
        ] {
            assert_eq!(icon(name), Some(shape), "{name}");
        }
    }

    /// A name that is no standard cursor's stays the X client's picture: a
    /// game's own, a blank one for hiding the pointer, a misspelling, and a
    /// name spelt in the wrong case, since X names are exact.
    #[test]
    fn a_name_that_is_no_standard_cursors_is_not_a_shape() {
        for name in ["", "none", "blank", "sword", "Left_Ptr", "XTERM", " xterm"] {
            assert_eq!(icon(name), None, "{name:?}");
        }
    }

    /// **XWayland coming up is what starts it**: the window manager's arm of
    /// `XWaylandEvent::Ready` tells the pointer which client XWayland is and
    /// starts the connection that hears the names.
    ///
    /// Asked of the source text, for the reason
    /// `a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is` gives:
    /// it needs a live XWayland, which this binary cannot start. What the
    /// pointer does once told is
    /// `state::tests::real_client::x11_cursor::xwaylands_cursor_surface_is_drawn_as_the_shape_its_x_name_stands_for`.
    #[test]
    fn xwayland_coming_up_starts_hearing_cursor_names() {
        let source = include_str!("../xwayland.rs");
        let start = source
            .find("        } => match X11Wm::start_wm::<D>(")
            .expect("the window manager is still started on `Ready`");
        let arm = source[start..]
            .find("            Err(err) => {")
            .map(|end| &source[start..start + end])
            .expect("and its arm still ends");
        assert!(
            arm.contains("solium.pointer.xwayland(client.id());")
                && arm.contains("crate::cursor::x11::watch(&loop_handle, display_number, theme);"),
            "XWayland comes up without the pointer hearing its cursor names:\n{arm}"
        );
    }

    /// Only XWayland's cursor surface stands for a shape, and only with a name
    /// known: a Wayland client's surface is its own pixels whatever the X
    /// cursor is called, as is XWayland's while the X cursor has no name, and
    /// a surface with no client is not XWayland's even in a session with no
    /// XWayland.
    #[test]
    fn only_xwaylands_cursor_surface_stands_for_a_shape() {
        let (xwayland, other) = (1, 2);
        let text = Some(CursorIcon::Text);
        assert_eq!(
            stands_for(Some(&xwayland), Some(&xwayland), text),
            text,
            "XWayland's surface, with a name known"
        );
        assert_eq!(
            stands_for(Some(&xwayland), Some(&xwayland), None),
            None,
            "XWayland's surface, with no name known"
        );
        assert_eq!(
            stands_for(Some(&other), Some(&xwayland), text),
            None,
            "a Wayland client's surface"
        );
        assert_eq!(
            stands_for::<i32>(None, None, text),
            None,
            "a surface whose client has gone, with no XWayland"
        );
        assert_eq!(
            stands_for(Some(&other), None, text),
            None,
            "any surface, with no XWayland"
        );
    }

    /// **#442: an unnamed X cursor that is a theme's picture is that
    /// shape**, and only an exact picture is: the same pixels at another
    /// hotspot, or one pixel different, is the client's own.
    #[test]
    fn an_unnamed_cursor_is_known_by_its_picture() {
        let arrow: Vec<u32> = (0..24 * 24).map(|pixel| 0xff00_0000 | pixel).collect();
        let mut pictures = Pictures::default();
        pictures.learn(Picture::of((24, 24), (4, 4), &arrow), CursorIcon::Default);
        pictures.learn(Picture::of((24, 24), (4, 4), &arrow), CursorIcon::Text);
        assert_eq!(
            pictures.shape(&Picture::of((24, 24), (4, 4), &arrow)),
            Some(CursorIcon::Default),
            "the picture Chromium built from the theme's file, and the first \
             theme to draw it names it"
        );
        assert_eq!(
            pictures.shape(&Picture::of((24, 24), (5, 4), &arrow)),
            None,
            "another hotspot"
        );
        let mut sword = arrow.clone();
        sword[100] = 0;
        assert_eq!(
            pictures.shape(&Picture::of((24, 24), (4, 4), &sword)),
            None,
            "a game's own cursor"
        );
    }

    /// **#442: a theme file's image is the picture X reports for it**: the
    /// file's little-endian ARGB words are the words `GetCursorImage` hands
    /// back, and the real theme on this machine, if there is one, knows its
    /// own arrow.
    #[test]
    fn a_theme_files_image_is_the_picture_x_reports() {
        let image = xcursor::parser::Image {
            size: 2,
            width: 2,
            height: 1,
            xhot: 1,
            yhot: 0,
            delay: 0,
            pixels_rgba: vec![0x10, 0x20, 0x30, 0xff, 0x01, 0x02, 0x03, 0x80],
            pixels_argb: Vec::new(),
        };
        assert_eq!(
            Picture::of_file(&image),
            Some(Picture::of((2, 1), (1, 0), &[0xff30_2010, 0x8003_0201])),
        );
        let adwaita = Pictures::load(&["Adwaita".to_owned()]);
        if adwaita.len() > 0 {
            let theme = xcursor::CursorTheme::load("Adwaita");
            let path = theme.load_icon("left_ptr").expect("Adwaita has an arrow");
            let images = xcursor::parser::parse_xcursor(&std::fs::read(path).expect("reading it"))
                .expect("parsing it");
            let first = Picture::of_file(&images[0]).expect("a picture");
            assert_eq!(adwaita.shape(&first), Some(CursorIcon::Default));
        }
    }

    /// **#442: the themes are Solium's, then the environment's, then the X
    /// resources', then GTK's, then `default`**, each once.
    #[test]
    fn the_themes_are_solium_s_then_the_environment_s_then_the_resources_then_gtk_s() {
        assert_eq!(
            themes(
                Some("Bibata"),
                Some("Adwaita"),
                Some("Xft.dpi:\t96\nXcursor.theme:\tAdwaita\nXcursor.size:\t24\n"),
                Some("breeze_cursors"),
            ),
            ["Bibata", "Adwaita", "breeze_cursors", "default"],
        );
        assert_eq!(
            themes(None, None, None, Some("breeze_cursors")),
            ["breeze_cursors", "default"],
            "Illia's: a QML pointer, and KDE's theme in GTK's settings"
        );
        assert_eq!(themes(None, Some(""), None, None), ["default"]);
        assert_eq!(
            resource_theme("Xcursor.size: 24\nXcursor.theme: Adwaita"),
            Some("Adwaita".to_owned())
        );
        assert_eq!(
            gtk_theme("[Settings]\ngtk-cursor-theme-name=breeze_cursors\n"),
            Some("breeze_cursors".to_owned()),
            "settings.ini"
        );
        assert_eq!(
            gtk_theme("gtk-cursor-theme-size=24\ngtk-cursor-theme-name=\"breeze_cursors\"\n"),
            Some("breeze_cursors".to_owned()),
            "~/.gtkrc-2.0"
        );
        assert_eq!(gtk_theme("gtk-theme-name=Breeze\n"), None);
    }
}
