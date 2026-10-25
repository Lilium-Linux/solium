//! What a notification carries, made safe to show (03 §3.2.11, "Content").
//!
//! Everything here comes from whichever client called `Notify`, so all of it
//! is hostile until proven otherwise: markup that would load remote images in
//! a QML `Text`, image data that is short, huge or row-padded, a path that is
//! not a file. Each rule is a pure function, with its limits, and nothing here
//! reaches the network or Qt.

use std::path::Path;

/// The largest side an image keeps. Larger ones are scaled down to fit:
/// `image_data_larger_than_the_cap_is_scaled_down_to_it`.
pub(crate) const IMAGE_MAX_SIDE: u32 = 1024;

/// The most bytes read from an `image-path`:
/// `an_image_path_over_the_size_cap_is_refused`.
pub(crate) const IMAGE_PATH_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// A body, twice: as plain text, and as markup a `Text.StyledText` can show
/// without loading anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Body {
    pub(crate) plain: String,
    pub(crate) rich: String,
}

/// The body markup the specification allows (`b`, `i`, `u`, `a href`), kept;
/// every other tag dropped with its text kept; entities decoded; `<`, `>` and
/// `&` in the text escaped again for the rich form. Markup that does not
/// parse is shown as the text it is, never as nothing.
/// `markup_keeps_bold_italic_underline_and_links`,
/// `an_image_tag_is_dropped_and_its_text_is_not`,
/// `broken_markup_is_shown_as_text`,
/// `entities_are_decoded_once`.
pub(crate) fn body(markup: &str) -> Body {
    let mut plain = String::new();
    let mut rich = String::new();
    let mut rest = markup;
    // Links opened and not yet closed: a `</a>` with none open is dropped, so
    // a link whose `<a>` was refused does not leave a stray close behind.
    let mut open_links = 0usize;
    while let Some(start) = rest.find('<') {
        push_text(&rest[..start], &mut plain, &mut rich);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            // A `<` that opens nothing: it is text.
            push_text(&rest[start..], &mut plain, &mut rich);
            rest = "";
            break;
        };
        let tag = &after[..end];
        if let Some(kept) = allowed_tag(tag) {
            if kept == "</a>" {
                if open_links > 0 {
                    open_links -= 1;
                    rich.push_str(&kept);
                }
            } else {
                if kept.starts_with("<a ") {
                    open_links += 1;
                }
                rich.push_str(&kept);
            }
        } else if tag.contains('<') {
            // `a < b > c`: what looked like a tag holds another `<`, so it was
            // never markup. Take the first `<` as text and go on from there.
            push_text("<", &mut plain, &mut rich);
            rest = after;
            continue;
        } else if tag
            .trim_matches(|c: char| c == '/' || c.is_whitespace())
            .eq_ignore_ascii_case("br")
        {
            plain.push('\n');
            rich.push_str("<br>");
        }
        rest = &after[end + 1..];
    }
    push_text(rest, &mut plain, &mut rich);
    for _ in 0..open_links {
        rich.push_str("</a>");
    }
    Body { plain, rich }
}

/// One of the four allowed tags, normalised, or `None`: `<a>` keeps only an
/// `href` with an `http`, `https` or `mailto` scheme, and every other
/// attribute (a `style`, an `onclick`) goes.
fn allowed_tag(tag: &str) -> Option<String> {
    let tag = tag.trim();
    let closing = tag.starts_with('/');
    let body = tag.trim_start_matches('/').trim();
    let name: String = body
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    match (name.as_str(), closing) {
        ("b" | "i" | "u", false) if body.len() == 1 => Some(format!("<{name}>")),
        ("b" | "i" | "u" | "a", true) => Some(format!("</{name}>")),
        ("a", false) => {
            let href = attribute(&body[1..], "href")?;
            let href = decode_entities(&href);
            let lower = href.to_ascii_lowercase();
            (lower.starts_with("https://")
                || lower.starts_with("http://")
                || lower.starts_with("mailto:"))
            .then(|| format!("<a href=\"{}\">", escape_attribute(&href)))
        }
        _ => None,
    }
}

/// An attribute's value, quoted with `"` or `'`.
fn attribute(attributes: &str, wanted: &str) -> Option<String> {
    let mut rest = attributes;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        let name_end = rest.find(|c: char| c == '=' || c.is_whitespace())?;
        let name = &rest[..name_end];
        rest = rest[name_end..].trim_start();
        let Some(after_equals) = rest.strip_prefix('=') else {
            continue;
        };
        rest = after_equals.trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let close = rest[1..].find(quote)? + 1;
        let value = &rest[1..close];
        rest = &rest[close + 1..];
        if name.eq_ignore_ascii_case(wanted) {
            return Some(value.to_owned());
        }
    }
}

fn push_text(text: &str, plain: &mut String, rich: &mut String) {
    let decoded = decode_entities(text);
    rich.push_str(&escape_text(&decoded));
    plain.push_str(&decoded);
}

/// `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;` and numeric references. An
/// unknown or broken entity stays as written.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let decoded = after.find(';').filter(|end| *end <= 10).and_then(|end| {
            let name = &after[..end];
            let character = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                    .and_then(char::from_u32)
                    .filter(|c| !c.is_control() || *c == '\n' || *c == '\t'),
            }?;
            Some((character, end))
        });
        match decoded {
            Some((character, end)) => {
                out.push(character);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("<br>"),
            c => out.push(c),
        }
    }
    out
}

fn escape_attribute(text: &str) -> String {
    escape_text(text).replace('"', "&quot;")
}

/// An image as straight RGBA, 8 bits a channel, rows packed.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Image {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

/// Why an image was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Zero or negative sides, or a stride too short for a row.
    Shape,
    /// Fewer bytes than the rows need.
    Short,
    /// Not 8 bits a sample, or not 3 or 4 channels.
    Format,
    /// Too many bytes to read.
    TooLarge,
    /// Not a file that could be read, or not one that decodes.
    Unreadable,
}

/// The `image-data` hint, `(iiibiiay)`: width, height, row stride, whether
/// there is alpha, bits per sample, channels and the bytes. Rows may be
/// padded past `width * channels`; the last one need not be. The result is
/// scaled to [`IMAGE_MAX_SIDE`].
/// `image_data_with_padded_rows_keeps_only_the_pixels`,
/// `image_data_shorter_than_its_rows_is_refused`,
/// `image_data_of_sixteen_bits_or_two_channels_is_refused`,
/// `image_data_larger_than_the_cap_is_scaled_down_to_it`.
pub(crate) fn image_data(
    width: i32,
    height: i32,
    stride: i32,
    has_alpha: bool,
    bits_per_sample: i32,
    channels: i32,
    bytes: &[u8],
) -> Result<Image, Refused> {
    let expected_channels: usize = if has_alpha { 4 } else { 3 };
    if bits_per_sample != 8 || usize::try_from(channels).ok() != Some(expected_channels) {
        return Err(Refused::Format);
    }
    let (Ok(width), Ok(height), Ok(stride)) = (
        usize::try_from(width),
        usize::try_from(height),
        usize::try_from(stride),
    ) else {
        return Err(Refused::Shape);
    };
    let channels = expected_channels;
    let row = width.checked_mul(channels).ok_or(Refused::Shape)?;
    if width == 0 || height == 0 || stride < row {
        return Err(Refused::Shape);
    }
    let needed = stride
        .checked_mul(height - 1)
        .and_then(|n| n.checked_add(row))
        .ok_or(Refused::Shape)?;
    if bytes.len() < needed {
        return Err(Refused::Short);
    }
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let line = &bytes[y * stride..y * stride + row];
        for pixel in line.chunks_exact(channels) {
            rgba.extend_from_slice(&pixel[..3]);
            rgba.push(if has_alpha { pixel[3] } else { 255 });
        }
    }
    let image = Image {
        width: u32::try_from(width).map_err(|_| Refused::Shape)?,
        height: u32::try_from(height).map_err(|_| Refused::Shape)?,
        rgba,
    };
    Ok(fit(image, IMAGE_MAX_SIDE))
}

/// The `image-path` hint, or an `app_icon` that is a path: a `file://` URL or
/// an absolute path, read only if it is a regular file no larger than
/// [`IMAGE_PATH_MAX_BYTES`], decoded as PNG, and scaled to
/// [`IMAGE_MAX_SIDE`]. `None` when the text is not a path at all, so the
/// caller treats it as an icon name.
/// `a_png_at_a_file_url_is_read_and_decoded`,
/// `an_image_path_over_the_size_cap_is_refused`,
/// `a_name_is_not_a_path`.
pub(crate) fn image_path(text: &str) -> Option<Result<Image, Refused>> {
    let path = text
        .strip_prefix("file://")
        .map(percent_decode)
        .or_else(|| text.starts_with('/').then(|| text.to_owned()))?;
    Some(read_png(Path::new(&path)))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_png(path: &Path) -> Result<Image, Refused> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    // Asked of the path first, and opened without blocking: a FIFO or a
    // device named here would otherwise hang the worker, and every
    // notification after it, for good.
    // `an_image_path_that_is_a_fifo_is_refused_without_waiting`.
    if !std::fs::metadata(path)
        .map_err(|_| Refused::Unreadable)?
        .is_file()
    {
        return Err(Refused::Unreadable);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| Refused::Unreadable)?;
    let metadata = file.metadata().map_err(|_| Refused::Unreadable)?;
    if !metadata.is_file() {
        return Err(Refused::Unreadable);
    }
    if metadata.len() > IMAGE_PATH_MAX_BYTES {
        return Err(Refused::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(IMAGE_PATH_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Refused::Unreadable)?;
    if bytes.len() as u64 > IMAGE_PATH_MAX_BYTES {
        return Err(Refused::TooLarge);
    }
    decode_png(&bytes)
}

/// PNG bytes to RGBA, any colour type and depth the decoder expands to 8-bit
/// samples, with a pixel budget so a tiny file cannot claim a huge canvas.
fn decode_png(bytes: &[u8]) -> Result<Image, Refused> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|_| Refused::Unreadable)?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    // 4096 x 4096 at most: past that, scaling down is not worth the memory.
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 4096 * 4096 {
        return Err(Refused::Shape);
    }
    let mut buffer = vec![0; reader.output_buffer_size().ok_or(Refused::Shape)?];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|_| Refused::Unreadable)?;
    let samples = &buffer[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => samples.to_vec(),
        png::ColorType::Rgb => samples
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => samples
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => samples.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err(Refused::Format),
    };
    Ok(fit(
        Image {
            width,
            height,
            rgba,
        },
        IMAGE_MAX_SIDE,
    ))
}

/// Scaled down, keeping its shape, so neither side is over `max`; averaged
/// over each target pixel's box, so a photo does not alias. Already small
/// enough: returned as it is.
fn fit(image: Image, max: u32) -> Image {
    if image.width <= max && image.height <= max {
        return image;
    }
    let scale = f64::from(max) / f64::from(image.width.max(image.height));
    let width = ((f64::from(image.width) * scale).round() as u32).clamp(1, max);
    let height = ((f64::from(image.height) * scale).round() as u32).clamp(1, max);
    let (sw, sh) = (image.width as usize, image.height as usize);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let y0 = y * sh / height as usize;
        let y1 = ((y + 1) * sh / height as usize).max(y0 + 1);
        for x in 0..width as usize {
            let x0 = x * sw / width as usize;
            let x1 = ((x + 1) * sw / width as usize).max(x0 + 1);
            let mut sum = [0u64; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let at = (sy * sw + sx) * 4;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u64::from(image.rgba[at + channel]);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            rgba.extend(sum.iter().map(|total| (total / count) as u8));
        }
    }
    Image {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_keeps_bold_italic_underline_and_links() {
        let body = body(
            "<b>Bold</b> <i>it</i> <u>u</u> <a href=\"https://x.org/?a=1&amp;b=2\" onclick=\"evil()\">link</a>",
        );
        assert_eq!(body.plain, "Bold it u link");
        assert_eq!(
            body.rich,
            "<b>Bold</b> <i>it</i> <u>u</u> <a href=\"https://x.org/?a=1&amp;b=2\">link</a>"
        );
    }

    #[test]
    fn an_image_tag_is_dropped_and_its_text_is_not() {
        let body = body(
            "look <img src=\"https://tracker.example/x.png\"/> here <span style=\"color:red\">red</span>",
        );
        assert_eq!(body.plain, "look  here red");
        assert!(!body.rich.contains("img"), "{}", body.rich);
        assert!(!body.rich.contains("tracker"), "{}", body.rich);
        assert!(!body.rich.contains("span"), "{}", body.rich);
    }

    #[test]
    fn a_link_with_a_script_scheme_loses_its_tag() {
        let body = body("<a href=\"javascript:alert(1)\">click</a>");
        assert_eq!(body.plain, "click");
        assert_eq!(body.rich, "click");
    }

    #[test]
    fn broken_markup_is_shown_as_text() {
        let body = body("1 < 2 and <b>bold");
        assert_eq!(body.plain, "1 < 2 and bold");
        assert_eq!(body.rich, "1 &lt; 2 and <b>bold");
        let lone = body_of("a < b");
        assert_eq!(lone.plain, "a < b");
        assert_eq!(lone.rich, "a &lt; b");
    }

    fn body_of(markup: &str) -> Body {
        body(markup)
    }

    #[test]
    fn entities_are_decoded_once() {
        let body = body("Tom &amp; Jerry &lt;3 &amp;lt; &#x263A; &#9731; &bogus;");
        assert_eq!(body.plain, "Tom & Jerry <3 &lt; \u{263A} \u{2603} &bogus;");
        assert_eq!(
            body.rich,
            "Tom &amp; Jerry &lt;3 &amp;lt; \u{263A} \u{2603} &amp;bogus;"
        );
    }

    #[test]
    fn newlines_and_br_become_line_breaks() {
        let body = body("one\ntwo<br/>three");
        assert_eq!(body.plain, "one\ntwo\nthree");
        assert_eq!(body.rich, "one<br>two<br>three");
    }

    #[test]
    fn image_data_with_padded_rows_keeps_only_the_pixels() {
        // 2x2 RGB, rows padded to 8 bytes; the last row unpadded.
        let bytes = [
            1, 2, 3, 4, 5, 6, 0xEE, 0xEE, //
            7, 8, 9, 10, 11, 12,
        ];
        let image = image_data(2, 2, 8, false, 8, 3, &bytes).expect("taken");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(
            image.rgba,
            vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }

    #[test]
    fn image_data_shorter_than_its_rows_is_refused() {
        assert_eq!(
            image_data(2, 2, 8, true, 8, 4, &[0; 15]),
            Err(Refused::Short)
        );
        assert_eq!(
            image_data(2, 2, 7, true, 8, 4, &[0; 64]),
            Err(Refused::Shape)
        );
        assert_eq!(
            image_data(0, 2, 8, true, 8, 4, &[0; 64]),
            Err(Refused::Shape)
        );
        assert_eq!(
            image_data(-1, 2, 8, true, 8, 4, &[0; 64]),
            Err(Refused::Shape)
        );
        // A huge canvas claimed with a few bytes.
        assert_eq!(
            image_data(100_000, 100_000, 400_000, true, 8, 4, &[0; 64]),
            Err(Refused::Short)
        );
        // A row wider than its stride.
        assert_eq!(
            image_data(i32::MAX, 2, i32::MAX, true, 8, 4, &[0; 64]),
            Err(Refused::Shape)
        );
    }

    #[test]
    fn image_data_of_sixteen_bits_or_two_channels_is_refused() {
        assert_eq!(
            image_data(1, 1, 8, true, 16, 4, &[0; 8]),
            Err(Refused::Format)
        );
        assert_eq!(
            image_data(1, 1, 2, false, 8, 2, &[0; 2]),
            Err(Refused::Format)
        );
        assert_eq!(
            image_data(1, 1, 4, false, 8, 4, &[0; 4]),
            Err(Refused::Format)
        );
    }

    #[test]
    fn image_data_larger_than_the_cap_is_scaled_down_to_it() {
        let (w, h) = (2048, 1024);
        let bytes = vec![200; (w * h * 4) as usize];
        let image = image_data(w, h, w * 4, true, 8, 4, &bytes).expect("taken");
        assert_eq!((image.width, image.height), (1024, 512));
        assert_eq!(image.rgba.len(), 1024 * 512 * 4);
        assert!(image.rgba.iter().all(|&b| b == 200));
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("header");
            let data = vec![9u8; (width * height * 3) as usize];
            writer.write_image_data(&data).expect("data");
        }
        out
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solium-notification-content-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_png_at_a_file_url_is_read_and_decoded() {
        let dir = scratch("png");
        let path = dir.join("a picture.png");
        std::fs::write(&path, png_bytes(3, 2)).expect("written");
        let url = format!("file://{}", path.display()).replace(' ', "%20");
        let image = image_path(&url).expect("a path").expect("decoded");
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(&image.rgba[..4], &[9, 9, 9, 255]);
        let plain = image_path(&path.display().to_string())
            .expect("a path")
            .expect("decoded");
        assert_eq!(plain, image);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_image_path_over_the_size_cap_is_refused() {
        let dir = scratch("big");
        let path = dir.join("big.png");
        std::fs::write(&path, vec![0u8; (IMAGE_PATH_MAX_BYTES + 1) as usize]).expect("written");
        assert_eq!(
            image_path(&path.display().to_string()),
            Some(Err(Refused::TooLarge))
        );
        assert_eq!(
            image_path(&dir.display().to_string()),
            Some(Err(Refused::Unreadable))
        );
        let junk = dir.join("junk.png");
        std::fs::write(&junk, b"not a png").expect("written");
        assert_eq!(
            image_path(&junk.display().to_string()),
            Some(Err(Refused::Unreadable))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_image_path_that_is_a_fifo_is_refused_without_waiting() {
        let dir = scratch("fifo");
        let fifo = dir.join("pipe.png");
        let path = std::ffi::CString::new(fifo.display().to_string()).expect("a path");
        // SAFETY: a valid NUL-terminated path.
        #[allow(unsafe_code)]
        let made = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(made, 0, "a fifo");
        assert_eq!(
            image_path(&fifo.display().to_string()),
            Some(Err(Refused::Unreadable))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_name_is_not_a_path() {
        assert_eq!(image_path("mail-unread"), None);
        assert_eq!(image_path("https://example.org/x.png"), None);
        assert_eq!(image_path(""), None);
    }
}
