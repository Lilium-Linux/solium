//! What a tray item sends, made safe to show: its pictures, its labels and
//! the icon files it names. Pure, but for reading those files: no bus and no
//! Qt.

use std::path::{Path, PathBuf};

use crate::services::notifications::content::{self, Image};

/// The largest side a picture is kept at, in pixels. A bar draws a tray icon
/// at a few dozen device pixels; past this a picture costs memory and shows
/// nothing more. With [`super::server::MAX_ITEMS`] items of three pictures
/// each, this is what bounds the pictures Qt holds, 12 MiB at most.
pub(crate) const ICON_MAX_SIDE: u32 = 128;

/// The side a picture is chosen for among those an item offers: the smallest
/// at least this large, so a bar at twice the scale stays sharp.
const WANTED_SIDE: u32 = 64;

/// A pixmap with a side past this is refused without being read.
const PIXMAP_MAX_SIDE: u32 = 1024;

/// The most text a label, a title or a tooltip's title keeps, and a
/// tooltip's body, in bytes.
pub(crate) const MAX_TEXT: usize = 256;
pub(crate) const MAX_TOOLTIP: usize = 4096;

/// The most sizes looked through under an `IconThemePath`'s `hicolor`.
const THEME_DIRS: usize = 64;

/// One of an item's `IconPixmap`, `AttentionIconPixmap` or
/// `OverlayIconPixmap`, `a(iiay)`: its sides and its ARGB32 pixels in network
/// byte order (A, R, G, B), as sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pixmap {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) argb: Vec<u8>,
}

/// The picture to show from the sizes an item offers: of those whose bytes
/// match their sides, the smallest at least [`WANTED_SIDE`], else the
/// largest; turned into straight RGBA and held to [`ICON_MAX_SIDE`]. `None`
/// when none is whole.
/// `the_pixmap_chosen_is_the_smallest_large_enough_and_its_bytes_are_turned_to_rgba`,
/// `a_pixmap_whose_bytes_do_not_match_its_sides_is_refused`.
pub(crate) fn pixmap(offered: &[Pixmap]) -> Option<Image> {
    let whole = offered.iter().filter_map(|pixmap| {
        let width = u32::try_from(pixmap.width).ok()?;
        let height = u32::try_from(pixmap.height).ok()?;
        let fits = width > 0
            && height > 0
            && width <= PIXMAP_MAX_SIDE
            && height <= PIXMAP_MAX_SIDE
            && pixmap.argb.len() == width as usize * height as usize * 4;
        fits.then_some((width.max(height), width, height, pixmap))
    });
    let chosen = whole.fold(None, |best: Option<(u32, u32, u32, &Pixmap)>, each| {
        let Some(best) = best else {
            return Some(each);
        };
        let better = match (best.0 >= WANTED_SIDE, each.0 >= WANTED_SIDE) {
            (true, true) => each.0 < best.0,
            (false, true) => true,
            (true, false) => false,
            (false, false) => each.0 > best.0,
        };
        Some(if better { each } else { best })
    })?;
    let (_, width, height, pixmap) = chosen;
    let rgba = pixmap
        .argb
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|[a, r, g, b]| [*r, *g, *b, *a])
        .collect();
    Some(content::fit(
        Image {
            width,
            height,
            rgba,
        },
        ICON_MAX_SIDE,
    ))
}

/// A short digest of a picture, to tell a new one from the one already
/// shown: an item that sends `NewIcon` for the same picture costs Qt
/// nothing.
pub(crate) fn digest(image: &Image) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    image.width.hash(&mut hasher);
    image.height.hash(&mut hasher);
    image.rgba.hash(&mut hasher);
    // Never 0, which reads as "no picture".
    hasher.finish().max(1)
}

/// A menu entry's label as it is shown: its mnemonic marks taken out (`_`
/// before the letter a key would pick, `__` for an underscore itself) and
/// held to [`MAX_TEXT`].
/// `a_label_loses_its_mnemonic_marks_and_keeps_a_doubled_underscore`.
pub(crate) fn label(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '_' {
            match chars.next() {
                Some(next) => shown.push(next),
                None => break,
            }
        } else {
            shown.push(c);
        }
    }
    cut(shown, MAX_TEXT)
}

/// A title or a tooltip as plain text: markup some items send (a tooltip's
/// body may hold a little HTML) taken out, entities decoded, and held to
/// `max` bytes.
/// `a_tooltip_s_markup_is_taken_out`.
pub(crate) fn plain(text: &str, max: usize) -> String {
    cut(content::body(&cut(text.to_owned(), max * 4)).plain, max)
}

/// `text`, at most `max` bytes, cut at a character boundary.
pub(crate) fn cut(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut at = max;
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        text.truncate(at);
    }
    text
}

/// The picture an `IconThemePath` holds for `name`, read: a PNG named for
/// it in the directory itself, as Qt and libappindicator write theirs, or
/// under its `hicolor/<size>x<size>/{apps,status}`, the size nearest above
/// [`WANTED_SIDE`] first. Only a plain icon name and an absolute directory
/// are looked at, and only a regular file is read, within the notification
/// images' size cap. `None` when it holds none.
/// `an_icon_theme_path_s_own_png_is_read_and_a_path_in_the_name_is_not`.
pub(crate) fn theme_file(theme_path: &str, name: &str) -> Option<Image> {
    let directory = Path::new(theme_path);
    if !directory.is_absolute()
        || theme_path.split('/').any(|part| part == "..")
        || crate::services::notifications::icon_name(name).is_empty()
    {
        return None;
    }
    let file = format!("{name}.png");
    let mut candidates = vec![directory.join(&file)];
    candidates.extend(
        sized_dirs(&directory.join("hicolor"))
            .into_iter()
            .flat_map(|dir| [dir.join("apps").join(&file), dir.join("status").join(&file)]),
    );
    candidates.into_iter().find_map(|path| {
        let text = path.to_str()?;
        content::image_path(text)?.ok()
    })
}

/// The `<size>x<size>` directories under `hicolor`, the smallest at least
/// [`WANTED_SIDE`] first, then the rest largest first.
fn sized_dirs(hicolor: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(hicolor) else {
        return Vec::new();
    };
    let mut sized: Vec<(u32, PathBuf)> = entries
        .take(THEME_DIRS)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let (side, other) = name.to_str()?.split_once('x')?;
            let side: u32 = side.parse().ok()?;
            (other.split('@').next()? == side.to_string()).then(|| (side, entry.path()))
        })
        .collect();
    sized.sort_by_key(|(side, _)| {
        if *side >= WANTED_SIDE {
            (0, *side)
        } else {
            (1, u32::MAX - side)
        }
    });
    sized.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(side: i32, argb: [u8; 4]) -> Pixmap {
        Pixmap {
            width: side,
            height: side,
            argb: argb.repeat((side * side) as usize),
        }
    }

    #[test]
    fn the_pixmap_chosen_is_the_smallest_large_enough_and_its_bytes_are_turned_to_rgba() {
        let offered = [
            square(16, [255, 1, 2, 3]),
            square(256, [255, 4, 5, 6]),
            square(64, [128, 10, 20, 30]),
            square(32, [255, 7, 8, 9]),
        ];
        let image = pixmap(&offered).expect("a picture");
        assert_eq!((image.width, image.height), (64, 64));
        assert_eq!(&image.rgba[..4], &[10, 20, 30, 128]);
        // With none large enough, the largest.
        let small = pixmap(&offered[..1]).expect("a picture");
        assert_eq!(small.width, 16);
        let both = pixmap(&[square(16, [0; 4]), square(48, [0; 4])]).expect("a picture");
        assert_eq!(both.width, 48);
        // Larger than kept: scaled down to the cap.
        let large = pixmap(&offered[1..2]).expect("a picture");
        assert_eq!((large.width, large.height), (ICON_MAX_SIDE, ICON_MAX_SIDE));
    }

    #[test]
    fn a_pixmap_whose_bytes_do_not_match_its_sides_is_refused() {
        let mut short = square(16, [255, 0, 0, 0]);
        short.argb.pop();
        assert_eq!(pixmap(&[short.clone()]), None);
        let negative = Pixmap {
            width: -4,
            height: 4,
            argb: vec![0; 64],
        };
        assert_eq!(pixmap(&[negative]), None);
        let huge = Pixmap {
            width: 2048,
            height: 1,
            argb: vec![0; 2048 * 4],
        };
        assert_eq!(pixmap(&[huge]), None);
        // A whole one beside a broken one is still taken.
        assert!(pixmap(&[short, square(8, [1, 2, 3, 4])]).is_some());
        assert_eq!(pixmap(&[]), None);
    }

    #[test]
    fn a_label_loses_its_mnemonic_marks_and_keeps_a_doubled_underscore() {
        assert_eq!(label("_Quit"), "Quit");
        assert_eq!(label("Show _Window"), "Show Window");
        assert_eq!(label("snake__case"), "snake_case");
        assert_eq!(label("trailing_"), "trailing");
        assert_eq!(label(&"é".repeat(200)).len(), MAX_TEXT);
    }

    #[test]
    fn a_tooltip_s_markup_is_taken_out() {
        assert_eq!(
            plain("<b>3</b> unread &amp; <img src=\"x\"/>new", MAX_TOOLTIP),
            "3 unread & new"
        );
        assert_eq!(plain("Steam", MAX_TEXT), "Steam");
    }

    #[test]
    fn an_icon_theme_path_s_own_png_is_read_and_a_path_in_the_name_is_not() {
        let dir = std::env::temp_dir().join(format!("solium-tray-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sized = dir.join("hicolor/22x22/apps");
        std::fs::create_dir_all(&sized).expect("a directory");
        let png = {
            let mut out = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut out, 1, 1);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().expect("header");
                writer.write_image_data(&[255, 0, 0]).expect("data");
            }
            out
        };
        std::fs::write(dir.join("steam_tray_mono.png"), &png).expect("written");
        std::fs::write(sized.join("chat.png"), &png).expect("written");
        let root = dir.to_str().expect("a path").to_owned();
        let image = theme_file(&root, "steam_tray_mono").expect("read");
        assert_eq!(&image.rgba, &[255, 0, 0, 255]);
        assert!(theme_file(&root, "chat").is_some(), "found under hicolor");
        assert_eq!(theme_file(&root, "../steam_tray_mono"), None);
        assert_eq!(theme_file(&root, "absent"), None);
        assert_eq!(theme_file("relative/dir", "steam_tray_mono"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
