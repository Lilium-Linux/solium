//! What a player sends, made safe to show: its text, and the album art its
//! `mpris:artUrl` names. Pure, but for reading a local art file: no bus and
//! no Qt, and nothing from the network.

use std::path::Path;

use crate::services::notifications::content::{self, Image, Refused};

/// The most a title, an artist, an album, an identity or a track id keeps,
/// in bytes.
pub(crate) const MAX_TEXT: usize = 512;
/// The most artists joined into `artist`.
pub(crate) const MAX_ARTISTS: usize = 16;
/// The longest art address handed to a scene, in bytes; a longer one is
/// left out.
pub(crate) const MAX_URL: usize = 2048;

/// The largest side art is kept at, in pixels. A now-playing card shows it at
/// a few hundred device pixels at most; past this it costs memory and shows
/// nothing more. With [`super::server::MAX_PLAYERS`] players, this bounds the
/// art Qt holds, 16 MiB at most.
pub(crate) const ART_MAX_SIDE: u32 = 512;

/// The largest `data:` URI read for art, in bytes: as large as a file read
/// for it, once base64 has grown it by a third.
pub(crate) const MAX_DATA_URI: usize = (content::IMAGE_PATH_MAX_BYTES as usize / 3 + 1) * 4 + 64;

/// Text as a player sent it, shown as plain text: control characters (a
/// newline in a title, an escape sequence) taken out, and held to
/// [`MAX_TEXT`]. `a_title_loses_its_control_characters_and_is_held_to_its_cap`.
pub(crate) fn text(sent: &str) -> String {
    let cleaned: String = sent
        .chars()
        .take(MAX_TEXT)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    crate::services::tray::content::cut(cleaned.trim().to_owned(), MAX_TEXT)
}

/// `xesam:artist`, a list, as one line: the first [`MAX_ARTISTS`] joined
/// with ", ", empty names left out.
pub(crate) fn artists<'a>(sent: impl IntoIterator<Item = &'a str>) -> String {
    let names: Vec<String> = sent
        .into_iter()
        .take(MAX_ARTISTS)
        .map(text)
        .filter(|name| !name.is_empty())
        .collect();
    crate::services::tray::content::cut(names.join(", "), MAX_TEXT)
}

/// Where `mpris:artUrl` points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// Read here: a `file://` URL or a `data:` URI.
    Local(String),
    /// Handed to the scene as it is.
    Remote(String),
    /// Nothing, or nothing Solium shows: another scheme, or too long.
    None,
}

/// What an art URL is, for [`read`] or for the scene.
/// `art_is_read_from_a_file_or_a_data_uri_and_handed_over_from_the_network`.
pub(crate) fn source(url: &str) -> Source {
    let url = url.trim();
    if url.starts_with("data:") {
        return if url.len() <= MAX_DATA_URI {
            Source::Local(url.to_owned())
        } else {
            Source::None
        };
    }
    if url.len() > MAX_URL || url.chars().any(char::is_control) {
        return Source::None;
    }
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    if lower.starts_with("file://") {
        Source::Local(url.to_owned())
    } else if lower.starts_with("https://") || lower.starts_with("http://") {
        Source::Remote(url.to_owned())
    } else {
        Source::None
    }
}

/// Art at a `file://` URL or in a `data:` URI, read and decoded (PNG or
/// JPEG) under the notification images' caps, and held to [`ART_MAX_SIDE`].
/// A file must be a regular, local one: a path inside a sandboxed player's
/// own filesystem is simply not there.
pub(crate) fn read(url: &str) -> Result<Image, Refused> {
    let bytes = if let Some(rest) = url.strip_prefix("data:") {
        let (kind, payload) = rest.split_once(',').ok_or(Refused::Format)?;
        if !kind.ends_with(";base64") {
            return Err(Refused::Format);
        }
        base64(payload).ok_or(Refused::Unreadable)?
    } else {
        let path = url
            .get(7..)
            .filter(|_| {
                url.get(..7)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
            })
            .map(content::percent_decode)
            .ok_or(Refused::Unreadable)?;
        // `file:///x` and `file://localhost/x`, and no other host.
        let path = path.strip_prefix("localhost").unwrap_or(&path).to_owned();
        if !path.starts_with('/') {
            return Err(Refused::Unreadable);
        }
        content::read_file(Path::new(&path))?
    };
    decode(&bytes).map(|image| content::fit(image, ART_MAX_SIDE))
}

/// PNG or JPEG bytes, by their first bytes, to RGBA.
fn decode(bytes: &[u8]) -> Result<Image, Refused> {
    if bytes.starts_with(b"\x89PNG") {
        content::decode_png(bytes)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        decode_jpeg(bytes)
    } else {
        Err(Refused::Format)
    }
}

/// JPEG bytes to RGBA, with the same pixel budget as a PNG.
fn decode_jpeg(bytes: &[u8]) -> Result<Image, Refused> {
    use zune_jpeg::zune_core::{
        bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
    };
    // 4096 x 4096 at most, as `decode_png`.
    const MAX_SIDE: usize = 4096;
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(MAX_SIDE)
        .set_max_height(MAX_SIDE);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().map_err(|_| Refused::Unreadable)?;
    let (width, height) = decoder.dimensions().ok_or(Refused::Shape)?;
    let size = decoder.output_buffer_size().ok_or(Refused::Shape)?;
    if width == 0 || height == 0 || size != width * height * 4 {
        return Err(Refused::Shape);
    }
    let mut rgba = vec![0; size];
    decoder
        .decode_into(&mut rgba)
        .map_err(|_| Refused::Unreadable)?;
    Ok(Image {
        width: u32::try_from(width).map_err(|_| Refused::Shape)?,
        height: u32::try_from(height).map_err(|_| Refused::Shape)?,
        rgba,
    })
}

/// Standard base64, padded or not, or `None` for anything else.
/// `base64_decodes_padded_and_unpadded_and_refuses_the_rest`.
fn base64(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 3);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x1 PNG: red, then green.
    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("header");
            writer
                .write_image_data(&[255, 0, 0, 0, 255, 0])
                .expect("data");
        }
        out
    }

    /// A 2x1 grey baseline JPEG.
    const JPEG: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoHBwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/2wBDAQMEBAUEBQkFBQkUDQsNFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBT/wAARCAABAAIDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwAooooA/9k=";

    fn encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        out
    }

    #[test]
    fn a_title_loses_its_control_characters_and_is_held_to_its_cap() {
        assert_eq!(text("Song\n\u{1b}[31mred"), "Song  [31mred");
        assert_eq!(text("  <b>plain</b> "), "<b>plain</b>", "markup stays text");
        assert_eq!(text(&"é".repeat(600)).len(), MAX_TEXT);
        assert_eq!(artists(["A", "", "B"]), "A, B");
        assert_eq!(
            artists(std::iter::repeat_n("x", 40)).len(),
            MAX_ARTISTS * 3 - 2
        );
    }

    #[test]
    fn base64_decodes_padded_and_unpadded_and_refuses_the_rest() {
        assert_eq!(base64("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(base64("aGVsbG8"), Some(b"hello".to_vec()));
        assert_eq!(base64("aGk="), Some(b"hi".to_vec()));
        assert_eq!(base64("a GVs"), None);
        assert_eq!(base64(&encode(&png())), Some(png()));
    }

    #[test]
    fn art_is_read_from_a_file_or_a_data_uri_and_handed_over_from_the_network() {
        assert_eq!(
            source("https://i.scdn.co/image/ab67"),
            Source::Remote("https://i.scdn.co/image/ab67".to_owned())
        );
        assert_eq!(
            source("file:///tmp/a.png"),
            Source::Local("file:///tmp/a.png".to_owned())
        );
        assert_eq!(source("ftp://host/a.png"), Source::None);
        assert_eq!(source("relative/a.png"), Source::None);
        assert_eq!(
            source(&format!("https://{}", "a".repeat(MAX_URL))),
            Source::None
        );
        assert_eq!(source(""), Source::None);

        let dir = std::env::temp_dir().join(format!("solium-media-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        let file = dir.join("cover art.png");
        std::fs::write(&file, png()).expect("written");
        let url = format!("file://{}", dir.join("cover%20art.png").display());
        let image = read(&url).expect("read");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(&image.rgba[..8], &[255, 0, 0, 255, 0, 255, 0, 255]);
        let jpeg = read(&format!("data:image/jpeg;base64,{JPEG}")).expect("decoded");
        assert_eq!((jpeg.width, jpeg.height), (2, 1));
        assert!((120..=136).contains(&jpeg.rgba[0]), "grey");
        assert_eq!(jpeg.rgba[3], 255);
        let data = read(&format!("data:image/png;base64,{}", encode(&png()))).expect("decoded");
        assert_eq!(data.width, 2);
        // Not an image, not base64, not a local file.
        std::fs::write(dir.join("notes.txt"), "hello").expect("written");
        assert!(read(&format!("file://{}", dir.join("notes.txt").display())).is_err());
        assert!(read("data:image/png,raw").is_err());
        assert!(read("file://elsewhere/tmp/a.png").is_err());
        assert!(read(&format!("file://{}", dir.join("absent.png").display())).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
