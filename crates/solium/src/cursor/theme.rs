//! The pointer as the rest of the machine draws it: an XCursor theme.
//!
//! `cursor.rs` next door draws Solium's own pointer from QML, deliberately, so
//! that the pointer belongs to the same design system as the window frames.
//! This module is the other half, and the reason it has to exist is that the
//! QML pointer is the only pointer the *compositor* can draw and it is not the
//! pointer anything else on the machine draws. `XCURSOR_THEME` and
//! `XCURSOR_SIZE` are what GTK, Qt and every toolkit follow, and until this
//! existed there was no way to make Solium's own pointer match them.
//!
//! It cuts both ways in one session. A client that sets its own cursor is
//! served by `CursorImageStatus::Surface` and gets whatever its toolkit
//! rasterised out of the *theme*; everything else got ours. So a session
//! showed two different pointer designs depending only on what the pointer
//! happened to be over, which reads as a glitch rather than as a choice.
//!
//! On a HiDPI screen the size is not cosmetic either. A pointer sized for 1x
//! on a 2x panel is half the size the user set everywhere else — on a 13" 4K
//! laptop that is a pointer which is genuinely hard to find, and this whole
//! area of the compositor exists because a pointer nobody can see is
//! indistinguishable from input being dead. See [`pixels`], which is the whole
//! of the scaling and is short on purpose.
//!
//! smithay ships no xcursor loader, so the *parsing* is the `xcursor` crate —
//! the same one winit and smithay's own anvil example use, and the same one
//! `wayland-cursor` is built on, so it is already in this tree's lockfile
//! through `wl-probe`. What is written here is the *choosing*: which theme,
//! how big, which of a file's several sizes, and — the part that matters most
//! — what happens when the answer to any of those is "there isn't one".
//!
//! **Nothing here resolves `wp_cursor_shape_v1`, and that is still the
//! design.** #24 landed on top of this and put the resolving in
//! [`super::shape`], which turns a named shape into an ordered list of
//! spellings and asks [`Theme::has`] about each in turn. This module answers
//! only "does this theme have a file called *that*". Which names to ask about
//! is a question about the protocol and about X11's vocabulary, not about the
//! theme, and keeping the two apart is what lets the whole shape-to-name
//! mapping be asserted with no theme on disk at all.

use std::collections::HashMap;

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Rectangle, Transform},
};

/// How big the pointer is when nobody has said, in logical pixels.
///
/// 24 is the size `cursor.qml` was drawn at and the size every desktop ships
/// as its own default, so a session that sets nothing looks the same as it did
/// before any of this existed.
pub(crate) const SIZE: i32 = 24;

/// The sizes a setting is allowed to name, in logical pixels.
///
/// Not taste. The pointer is held as a square buffer per output scale, so this
/// number is multiplied by the scale and then squared: 1024 logical pixels at
/// 2x is a 16 MB pointer, kept, per name. The floor is 1 rather than 0 because
/// a zero-sized buffer allocates cleanly, uploads cleanly and draws nothing —
/// an invisible pointer arrived at without a single error, which is the exact
/// failure this module's neighbour was written to stop happening.
const SIZES: std::ops::RangeInclusive<i32> = 1..=256;

/// The largest scale a monitor may be configured at; see `config.lua`.
///
/// Only used to bound [`pixels`]. Restated rather than shared because the
/// monitor code clamps a *scale* and this clamps a *buffer edge*, and the day
/// those two want different numbers is the day sharing one constant would have
/// been the bug.
const MAX_SCALE: i32 = 8;

/// What `config.lua` said about the pointer.
///
/// Every field is `Option` and `None` means "the configuration did not say",
/// which is not the same as "off" — it is what lets the environment be
/// consulted next. See [`Settings::resolve`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Configured {
    pub(crate) theme: Option<String>,
    pub(crate) size: Option<i32>,
}

/// `XCURSOR_THEME` and `XCURSOR_SIZE` as this process sees them.
///
/// A struct read once and passed in, rather than [`Settings::resolve`] calling
/// `std::env::var` itself, and the reason is the tests. Precedence is the
/// thing most worth pinning here and pinning it by *setting* environment
/// variables would be a test that mutates process-global state — which in
/// edition 2024 is `unsafe` and, more to the point, races every other test in
/// the same binary. So the environment is a value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Environment {
    pub(crate) theme: Option<String>,
    /// Raw, and parsed by [`Settings::resolve`] rather than here, so that
    /// `XCURSOR_SIZE=enormous` is a case the precedence tests can state.
    pub(crate) size: Option<String>,
}

impl Environment {
    /// What this process was started with.
    pub(crate) fn read() -> Self {
        // Empty is treated as unset, which is what the `xcursor` crate does
        // with the search-path variables and what a shell leaves behind after
        // `XCURSOR_THEME=` on a line of its own.
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            theme: read("XCURSOR_THEME"),
            size: read("XCURSOR_SIZE"),
        }
    }
}

/// The pointer's theme and size, after every source has been consulted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// The theme to draw from, or `None` for Solium's own QML pointer.
    pub(crate) theme: Option<String>,
    /// How big the pointer is, in **logical** pixels. See [`pixels`].
    pub(crate) size: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: None,
            size: SIZE,
        }
    }
}

impl Settings {
    /// Three sources, in this order, and the order is the whole design:
    ///
    /// 1. **`config.lua`.** An explicit setting wins, because someone wrote it
    ///    for *this* compositor and meant it. It is also the only source that
    ///    can be reloaded with `super+shift+r`.
    /// 2. **`XCURSOR_THEME` and `XCURSOR_SIZE`.** What the rest of the machine
    ///    follows: a session that exports them from `~/.profile` or from a
    ///    display manager has already told GTK, Qt and every toolkit what the
    ///    pointer is. A compositor that ignored them would be the one thing on
    ///    screen drawing a different pointer from everything else — which is
    ///    precisely the split-personality session this module exists to close.
    /// 3. **The built-in default:** no theme, and [`SIZE`] logical pixels. No
    ///    theme means the QML pointer, which is the one arm that cannot fail
    ///    for want of a file on disk.
    ///
    /// The two settings are resolved *independently*. A configuration that
    /// names a size and no theme takes `XCURSOR_THEME` from the environment
    /// and its own size, which is the useful case — "the theme everything else
    /// uses, but bigger" — and would be lost if the config table were taken or
    /// ignored whole.
    pub(crate) fn resolve(configured: &Configured, environment: &Environment) -> Self {
        let theme = configured
            .theme
            .clone()
            .or_else(|| environment.theme.clone())
            .filter(|name| !name.is_empty());

        // A size that is out of range is *dropped*, not clamped, so the next
        // source still gets its turn. `XCURSOR_SIZE=0` left by some other
        // session's startup script should leave the built-in 24 standing
        // rather than silently producing a one-pixel pointer, and a clamp here
        // would produce the one-pixel pointer.
        let size = configured
            .size
            .and_then(|size| sane(size, "config.lua"))
            .or_else(|| {
                environment.size.as_ref().and_then(|raw| {
                    let parsed = raw.parse::<i32>().ok();
                    if parsed.is_none() {
                        tracing::warn!(
                            size = %raw,
                            "XCURSOR_SIZE is not a number; using the default pointer size"
                        );
                    }
                    parsed.and_then(|size| sane(size, "XCURSOR_SIZE"))
                })
            })
            .unwrap_or(SIZE);

        Self { theme, size }
    }
}

/// A size within [`SIZES`], or nothing and a line saying whose it was.
///
/// `where_from` is in the message because "cursor size 0 refused" with no
/// source sends the reader to the wrong file: the two places this is called
/// from are a file the user wrote and an environment variable they may not
/// know is set.
fn sane(size: i32, where_from: &str) -> Option<i32> {
    if SIZES.contains(&size) {
        return Some(size);
    }
    tracing::warn!(
        size,
        source = where_from,
        min = *SIZES.start(),
        max = *SIZES.end(),
        "cursor size out of range; ignoring it"
    );
    None
}

/// The pointer's size in device pixels, on an output at `scale`.
///
/// **This is the multiplication the issue calls out as not cosmetic**, so it is
/// worth saying plainly why it is here and not in [`Settings`]. The size is
/// *logical*, exactly as every radius and inset in this codebase is logical,
/// and it is scaled at the point of use. That is not a stylistic echo: a
/// compositor drives several outputs at several scales *at once* and the
/// pointer crosses between them, so there is no single device size it could
/// have been stored as. `render::cursor` builds the pointer for every output
/// every frame precisely so that the one straddling a boundary appears
/// correctly on both, and each of those calls arrives here with that output's
/// own scale.
///
/// A bug in this line is invisible at 1x — `scale` is 1.0 and the two numbers
/// agree — and wrong on every HiDPI screen, which is why the tests below pin
/// it at 2x and at 1.5x rather than at 1x.
pub(crate) fn pixels(size: i32, scale: f64) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to SIZES * MAX_SCALE on the next line"
    )]
    let scaled = (f64::from(size) * scale).round() as i32;
    // Clamped rather than trusted: `scale` reaches here from an output, and a
    // NaN or a wild value would otherwise become an allocation. `clamp` on a
    // value that started as NaN would panic, so the cast above is allowed to
    // saturate first — `as i32` of NaN is 0, which the floor then lifts to 1.
    scaled.clamp(*SIZES.start(), *SIZES.end() * MAX_SCALE)
}

/// One cursor from a theme, rasterised and ready to upload.
#[derive(Debug)]
pub(crate) struct Ready {
    /// Premultiplied ARGB8888, which is what smithay will put on a hardware
    /// cursor plane. See [`Ready::from_image`] for why no conversion is needed.
    pub(crate) buffer: MemoryRenderBuffer,
    /// The image's real size in device pixels, which is **not** necessarily
    /// the size that was asked for: a theme has the sizes its author drew.
    pub(crate) size: (i32, i32),
    /// Where in those pixels the point of the cursor is.
    ///
    /// The theme's, unlike the QML pointer's hardcoded `(0, 0)` — an I-beam
    /// points from its middle and a resize arrow from its centre, so drawing a
    /// themed cursor at its plain location would put every one of them wrong
    /// except the arrow.
    pub(crate) hotspot: (i32, i32),
}

impl Ready {
    /// Turn one parsed XCursor image into a buffer, or nothing if it is
    /// malformed.
    ///
    /// **`pixels_rgba` and not `pixels_argb`, and the field names are the
    /// trap.** An XCursor file stores each pixel as a 32-bit *little-endian*
    /// word in ARGB order, so the bytes as they sit in the file are B, G, R,
    /// A. That is exactly what DRM means by `ARGB8888` — a little-endian
    /// 32-bit ARGB word — and exactly what `MemoryRenderBuffer` wants, so the
    /// file's own bytes go in untouched. The crate calls that field
    /// `pixels_rgba` because it is "the order of the file", and its
    /// `pixels_argb` re-orders those bytes to A, B, G, R, which is not a
    /// format anything here can use. `wayland-cursor` reaches the same
    /// conclusion the same way: `lib.rs:367` writes `pixels_rgba` into a
    /// buffer declared `Format::Argb8888`.
    ///
    /// Alpha is premultiplied already — libxcursor's format says so and every
    /// theme on disk obeys it — which is the other thing `MemoryRenderBuffer`
    /// assumes and the other thing that would be silently, subtly wrong.
    fn from_image(image: &xcursor::parser::Image) -> Option<Self> {
        let width = i32::try_from(image.width).ok()?;
        let height = i32::try_from(image.height).ok()?;
        if width <= 0 || height <= 0 {
            return None;
        }
        let row = usize::try_from(width).ok()?.checked_mul(4)?;
        let rows = usize::try_from(height).ok()?;
        let wanted = row.checked_mul(rows)?;
        // A file claiming a size it does not carry pixels for is the "malformed
        // cursor file" case, and it degrades: this cursor is not available, the
        // caller falls back, and nothing aborts.
        if image.pixels_rgba.len() < wanted {
            return None;
        }

        let mut buffer = MemoryRenderBuffer::new(
            Fourcc::Argb8888,
            (width, height),
            1,
            Transform::Normal,
            None,
        );
        let mut context = buffer.render();
        let copy = context.draw(|target| {
            let Some(target) = target.get_mut(..wanted) else {
                return Err(());
            };
            target.copy_from_slice(&image.pixels_rgba[..wanted]);
            Ok(vec![Rectangle::from_size((width, height).into())])
        });
        drop(context);
        copy.ok()?;

        Some(Self {
            buffer,
            size: (width, height),
            hotspot: (
                i32::try_from(image.xhot).unwrap_or_default(),
                i32::try_from(image.yhot).unwrap_or_default(),
            ),
        })
    }
}

/// A cursor theme installed on this machine.
#[derive(Debug)]
pub(crate) struct Theme {
    /// What it is called, for the log lines. The `xcursor` crate keeps its own
    /// copy and does not lend it out.
    name: String,
    theme: xcursor::CursorTheme,
    /// One answer per cursor name and device size, **including the negative
    /// one**.
    ///
    /// `None` under a key means "this theme has no such cursor at this size",
    /// and keeping that is the point rather than an accident. Without it, a
    /// theme missing `grab` would walk the whole icon search path — every
    /// inherited theme, every directory in `XCURSOR_PATH` — on every frame for
    /// as long as a window is being dragged, to arrive at the same answer each
    /// time.
    ///
    /// Unbounded, unlike `Kept` next door, and that is a judgment rather than
    /// an oversight: the keys are cursor *names*, of which the whole w3c set
    /// is thirty-six, times the handful of device sizes a machine's monitors
    /// produce. Thirty-six 48-pixel cursors is about 330 KB. A cache with a
    /// cap here would evict entries that will certainly be asked for again.
    ready: HashMap<(String, i32), Option<Ready>>,
}

impl Theme {
    /// Find the named theme, or nothing.
    ///
    /// **`xcursor::CursorTheme::load` cannot fail**, and that is the thing to
    /// know about it: handed a name nothing on this machine answers to, it
    /// returns a theme with no directories under it, and every lookup on that
    /// theme then answers `None`. So "did the theme load" is not a question
    /// the crate will answer and has to be asked as "does it have a pointer" —
    /// asked here, once, rather than discovered one missing shape at a time
    /// with no line in the log to say why every cursor is Solium's own.
    ///
    /// Two names because a theme's arrow is called `default` if it was written
    /// to the w3c names and `left_ptr` if it was written to the X11 ones, and
    /// plenty of installed themes are still the latter.
    pub(crate) fn load(name: &str) -> Option<Self> {
        let theme = xcursor::CursorTheme::load(name);
        if theme.load_icon("default").is_none() && theme.load_icon("left_ptr").is_none() {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            theme,
            ready: HashMap::new(),
        })
    }

    /// The cursor called exactly `name`, at `pixels` device pixels.
    ///
    /// One name and no alternatives, because choosing between spellings is
    /// [`super::shape`]'s job and not this one's: by the time a name reaches
    /// here it is the one [`super::shape::resolve`] found this theme to have,
    /// which is why the [`Theme::has`] call below is a cache hit rather than a
    /// second directory walk.
    ///
    /// Takes `&str` and not a `CursorIcon` for the reason the module header
    /// gives — a theme knows about files, not about protocol enumerations.
    ///
    /// `None` is an ordinary answer and not an error: it means this theme does
    /// not have this cursor, and the caller draws Solium's own instead.
    pub(crate) fn ready(&mut self, name: &str, pixels: i32) -> Option<&Ready> {
        // Resolved first and looked up second, rather than one pass that both
        // rasterises and returns a borrow: `has` takes `&mut self` and the
        // borrow it would leave behind outlives the `if`.
        if !self.has(name, pixels) {
            return None;
        }
        self.ready.get(&(name.to_owned(), pixels))?.as_ref()
    }

    /// Whether this theme has `name` at `pixels`, doing the work once.
    ///
    /// **This is the predicate [`super::shape::resolve`] walks a chain of
    /// spellings with**, which is why it is `pub(crate)` and separate from
    /// [`Theme::ready`]: the caller has to be able to ask about four or five
    /// names and take the first that answers, and a method that handed back a
    /// borrow could only be asked once.
    ///
    /// Deliberately does not need a renderer, and that is what makes the
    /// fallback testable: turning an xcursor file into a `MemoryRenderBuffer`
    /// is a file read and a memcpy, so the whole of "does this theme have this
    /// cursor at this size" can be asked — and asserted, in a unit test, on a
    /// machine with no GPU and no cursor themes at all — without one. Only
    /// `Pointer::themed` needs a renderer, and only to upload.
    pub(crate) fn has(&mut self, name: &str, pixels: i32) -> bool {
        let key = (name.to_owned(), pixels);
        if let Some(known) = self.ready.get(&key) {
            return known.is_some();
        }
        let made = self.make(name, pixels);
        let has = made.is_some();
        if !has {
            tracing::debug!(
                theme = %self.name,
                cursor = name,
                pixels,
                "the cursor theme has no such cursor"
            );
        }
        self.ready.insert(key, made);
        has
    }

    /// Read one cursor off disk and pick the size closest to `pixels`.
    ///
    /// Every failure on the way is a `None` and none of them is loud. A theme
    /// that is missing one shape, a file that has been truncated, a directory
    /// that turned out to be unreadable — all of them mean the same thing to
    /// the caller, which is "draw ours instead", and all of them are
    /// per-cursor rather than per-session. The one-line `debug!` in
    /// [`Theme::rasterised`] is the whole of what they are worth; the loud line
    /// belongs at [`Theme::load`], where a missing *theme* is diagnosed once.
    fn make(&self, name: &str, pixels: i32) -> Option<Ready> {
        let path = self.theme.load_icon(name)?;
        let content = std::fs::read(&path).ok()?;
        let images = xcursor::parser::parse_xcursor(&content)?;
        Ready::from_image(nearest(&images, pixels)?)
    }
}

/// The image nearest `pixels`, preferring the larger when two are equally far.
///
/// A theme has the sizes its author drew — commonly 24, 32, 48 and 64 in one
/// file — so an exact match is the usual case and not a guaranteed one. The
/// tie goes to the larger because a cursor scaled *down* keeps its shape and
/// one scaled up does not, and a tie only happens at the midpoint between two
/// sizes where the choice is otherwise arbitrary.
///
/// **An animated cursor arrives here as several images at the same size**, one
/// per frame, each with its own `delay`. `min_by_key` returns the first of
/// equal keys, so this takes frame one and the pointer is still. That is a
/// deliberate limit and not an oversight: animating it would mean a timer and
/// a redraw of every output at the theme's own frame rate for something nobody
/// looks at, and the first frame of a busy cursor is a recognisable busy
/// cursor.
fn nearest(images: &[xcursor::parser::Image], pixels: i32) -> Option<&xcursor::parser::Image> {
    images.iter().min_by_key(|image| {
        let size = i64::from(image.size);
        ((size - i64::from(pixels)).abs(), -size)
    })
}

/// A theme name no machine can have installed, for the tests — here and in
/// `cursor.rs` — that are about *not* finding one.
///
/// Spelled out rather than generated so that a failure is greppable, and
/// deliberately not a plausible theme name: a test that passed because the
/// machine happened not to have "Adwaita" installed would start failing on the
/// machine that does.
#[cfg(test)]
pub(crate) const NOT_INSTALLED: &str = "solium-test-theme-that-cannot-exist-4f2a91";

#[cfg(test)]
mod tests {
    use smithay::input::pointer::CursorIcon;

    use crate::cursor::shape;

    use super::{
        Configured, Environment, NOT_INSTALLED, Ready, SIZE, Settings, Theme, nearest, pixels,
    };

    fn configured(theme: Option<&str>, size: Option<i32>) -> Configured {
        Configured {
            theme: theme.map(str::to_owned),
            size,
        }
    }

    fn environment(theme: Option<&str>, size: Option<&str>) -> Environment {
        Environment {
            theme: theme.map(str::to_owned),
            size: size.map(str::to_owned),
        }
    }

    /// The precedence, stated once as the three-way case it is.
    ///
    /// Config beats environment beats default, and all three sources are
    /// present so that a regression which drops a level shows up as a wrong
    /// answer rather than as an absent one.
    #[test]
    fn the_configuration_wins_over_the_environment() {
        let settings = Settings::resolve(
            &configured(Some("Configured"), Some(48)),
            &environment(Some("FromTheEnvironment"), Some("32")),
        );
        assert_eq!(settings.theme.as_deref(), Some("Configured"));
        assert_eq!(settings.size, 48);
    }

    /// And with nothing in `config.lua`, the environment is what the rest of
    /// the machine already follows, so it is what the pointer follows too.
    #[test]
    fn the_environment_wins_over_the_default() {
        let settings = Settings::resolve(
            &Configured::default(),
            &environment(Some("FromTheEnvironment"), Some("32")),
        );
        assert_eq!(settings.theme.as_deref(), Some("FromTheEnvironment"));
        assert_eq!(settings.size, 32);
    }

    /// With neither, the built-in default — and the built-in default is *no
    /// theme*, which is what keeps the QML pointer the thing that is drawn on
    /// a machine that has said nothing.
    #[test]
    fn nothing_anywhere_is_the_built_in_default() {
        let settings = Settings::resolve(&Configured::default(), &Environment::default());
        assert_eq!(settings.theme, None);
        assert_eq!(settings.size, SIZE);
    }

    /// The two settings resolve independently: a size in `config.lua` must not
    /// take the theme out of the environment with it.
    ///
    /// This is the useful mixed case — "the theme everything else uses, but
    /// bigger" — and the one an implementation that took the config table
    /// whole or ignored it whole would get wrong.
    #[test]
    fn a_configured_size_leaves_the_environments_theme_standing() {
        let settings = Settings::resolve(
            &configured(None, Some(64)),
            &environment(Some("FromTheEnvironment"), Some("32")),
        );
        assert_eq!(settings.theme.as_deref(), Some("FromTheEnvironment"));
        assert_eq!(settings.size, 64);
    }

    /// A size that is not a size falls through to the next source rather than
    /// being clamped into one.
    ///
    /// `XCURSOR_SIZE=0` is left behind by more than one desktop's startup
    /// scripts. Clamping it to 1 would give a one-pixel pointer, arrived at
    /// with no error anywhere, which is the invisible-pointer failure wearing
    /// a different hat.
    #[test]
    fn an_impossible_size_falls_through() {
        assert_eq!(
            Settings::resolve(&Configured::default(), &environment(None, Some("0"))).size,
            SIZE
        );
        assert_eq!(
            Settings::resolve(&Configured::default(), &environment(None, Some("enormous"))).size,
            SIZE
        );
        assert_eq!(
            Settings::resolve(&configured(None, Some(-4)), &environment(None, Some("32"))).size,
            32,
            "a refused configured size still leaves the environment its turn"
        );
    }

    /// The scale multiplication, at scales that are not 1.
    ///
    /// Pinned at 2x and 1.5x because the bug this guards against is invisible
    /// at 1x: a `pixels` that ignored `scale` entirely would pass a 1x test
    /// and halve the pointer on every HiDPI screen.
    #[test]
    fn the_size_is_multiplied_by_the_output_scale() {
        assert_eq!(pixels(24, 1.0), 24);
        assert_eq!(pixels(24, 2.0), 48, "a 2x monitor needs twice the pixels");
        assert_eq!(pixels(24, 1.5), 36, "and a fractional scale is exact here");
        assert_eq!(pixels(32, 1.25), 40);
        assert_eq!(
            pixels(24, 1.3),
            31,
            "half-pixels round rather than truncate"
        );
    }

    /// And a scale that is not a number cannot become an allocation.
    #[test]
    fn a_nonsense_scale_still_gives_a_drawable_size() {
        assert!(pixels(24, f64::NAN) >= 1);
        assert!(pixels(24, f64::INFINITY) <= 256 * 8);
        assert!(pixels(24, -3.0) >= 1);
    }

    /// A theme nothing answers to loads as nothing, rather than as an empty
    /// theme every lookup then quietly fails against.
    ///
    /// The other half of this — that the caller then draws the QML pointer
    /// rather than nothing at all — is `cursor::tests`, which has the
    /// `Pointer` to assert it on.
    #[test]
    fn a_theme_that_is_not_installed_does_not_load() {
        assert!(
            Theme::load(NOT_INSTALLED).is_none(),
            "a name nothing on disk answers to produced a theme"
        );
    }

    /// And a theme that really is installed loads and hands back a cursor.
    ///
    /// **Machine-dependent, and skipped rather than failed when this machine
    /// has no cursor themes at all** — which is a real configuration and the
    /// one every other test here is about. The same guard `script.rs` puts on
    /// its `~/.config/solium` test, and for the same reason: a test whose
    /// subject is the disk has to say so rather than fail on a build box.
    ///
    /// What it is worth is that it is the only thing covering the *whole* disk
    /// path in one go — resolve the shape's spellings, search the icon
    /// directories, follow what a theme inherits, read the file, pick a size,
    /// copy the bytes in the order DRM wants them. Every other test here and
    /// in `super::shape` stubs one piece of that.
    #[test]
    fn an_installed_theme_hands_back_a_cursor() {
        // "default" is the redirect theme nearly every distribution installs
        // and the two named after it are the GNOME and KDE ones; if none of
        // the three is here, this machine genuinely has no cursor themes.
        let Some(mut found) = ["default", "Adwaita", "breeze_cursors"]
            .into_iter()
            .find_map(Theme::load)
        else {
            return;
        };
        // Through `shape::resolve` rather than a name picked here, so that this
        // exercises the same walk a client naming a shape takes.
        let name = shape::resolve(CursorIcon::Default, |candidate| found.has(candidate, 24))
            .expect("an installed theme has an arrow under one of its names");
        let ready = found
            .ready(name, 24)
            .expect("the name the walk just found is one this theme has");
        assert!(ready.size.0 > 0 && ready.size.1 > 0);
        // The hotspot is inside the image, which is the invariant a cursor
        // drawn at the wrong offset would break.
        assert!(ready.hotspot.0 <= ready.size.0 && ready.hotspot.1 <= ready.size.1);
    }

    /// And a real theme really does answer to a legacy spelling for a shape it
    /// has never heard the modern name of.
    ///
    /// Machine-dependent and skipped the same way, and worth having anyway:
    /// every other assertion about the fallback chain is against a closure we
    /// wrote, so this is the only place the chain meets a directory of files
    /// somebody else laid out. `Text` is the one to ask about because `xterm`
    /// is present in effectively every theme ever shipped.
    #[test]
    fn an_installed_theme_answers_a_shape_under_whichever_name_it_has() {
        let Some(mut found) = ["default", "Adwaita", "breeze_cursors"]
            .into_iter()
            .find_map(Theme::load)
        else {
            return;
        };
        let name = shape::resolve(CursorIcon::Text, |candidate| found.has(candidate, 24));
        assert!(
            name.is_some(),
            "no spelling of the I-beam was found in an installed theme"
        );
    }

    /// One image per nominal size, as a theme file holds them.
    fn image(size: u32, hot: u32) -> xcursor::parser::Image {
        let pixels = usize::try_from(size).unwrap_or_default().pow(2) * 4;
        xcursor::parser::Image {
            size,
            width: size,
            height: size,
            xhot: hot,
            yhot: hot,
            delay: 0,
            pixels_rgba: vec![0x80; pixels],
            pixels_argb: vec![0x80; pixels],
        }
    }

    /// Which of a file's sizes is taken, including the tie.
    #[test]
    fn the_nearest_size_is_taken_and_a_tie_goes_up() {
        let images = [image(24, 1), image(32, 2), image(48, 3), image(64, 4)];
        assert_eq!(nearest(&images, 48).map(|found| found.size), Some(48));
        assert_eq!(nearest(&images, 50).map(|found| found.size), Some(48));
        assert_eq!(
            nearest(&images, 28).map(|found| found.size),
            Some(32),
            "equally far from 24 and 32, so the larger"
        );
        assert_eq!(
            nearest(&images, 4096).map(|found| found.size),
            Some(64),
            "past the largest it has, so the largest it has"
        );
        assert_eq!(nearest(&[], 24).map(|found| found.size), None);
    }

    /// A file that claims more pixels than it carries is refused rather than
    /// read past.
    ///
    /// This is the "malformed cursor file" arm, and what it must produce is a
    /// `None` the caller can fall back from — not a panic, which the workspace
    /// lints deny, and not a buffer of whatever was next in memory.
    #[test]
    fn a_truncated_image_is_refused() {
        let mut truncated = image(24, 1);
        truncated.pixels_rgba.truncate(17);
        assert!(Ready::from_image(&truncated).is_none());

        let whole = image(24, 1);
        let ready = Ready::from_image(&whole).expect("a whole image is readable");
        assert_eq!(ready.size, (24, 24));
        assert_eq!(ready.hotspot, (1, 1), "the theme's hotspot, not (0, 0)");
    }
}
