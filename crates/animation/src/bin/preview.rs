//! Build the animation preview page.
//!
//! Embeds two engines — the animation curves and the window arrangements, both
//! compiled to WebAssembly — into `preview/index.html`, and writes the result.
//! The page calls them directly, so what is tuned in a browser is the code that
//! moves and arranges real windows. A reimplementation in JavaScript would
//! drift the first time either side changed, and the drift would be invisible
//! because the page would still look plausible.
//!
//! Driven by `dev/preview`, which builds the wasm first.
//!
//! ```sh
//! dev/preview && xdg-open crates/animation/preview/preview.html
//! ```

use std::{io::Write as _, path::PathBuf};

use solium_animation::Curve;

fn main() {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let (Some(animation), Some(layout)) = (args.next(), args.next()) else {
        eprintln!("usage: preview <animation.wasm> <layout.wasm>");
        eprintln!("       run dev/preview instead, which builds them first");
        std::process::exit(2);
    };

    let read = |path: &PathBuf| match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("error: could not read {}: {err}", path.display());
            std::process::exit(1);
        }
    };
    let bytes = read(&animation);
    let layout_bytes = read(&layout);

    let names: Vec<String> = Curve::all()
        .into_iter()
        .map(|(name, _)| format!("{name:?}"))
        .collect();

    const PAGE: &str = include_str!("../../preview/index.html");
    let page = PAGE
        .replace("/*CURVES*/ null", &format!("[{}]", names.join(", ")))
        .replace("/*WASM*/ \"\"", &format!("{:?}", base64(&bytes)))
        .replace("/*LAYOUT*/ \"\"", &format!("{:?}", base64(&layout_bytes)));

    let mut stdout = std::io::stdout().lock();
    if let Err(err) = stdout.write_all(page.as_bytes()) {
        eprintln!("error: could not write the page: {err}");
        std::process::exit(1);
    }
}

/// Base64, so the wasm can live inside the page.
///
/// Hand-rolled rather than pulled in: this crate must stay dependency-free so
/// it keeps building for the browser, and this is twenty lines.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0_u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);

        for slot in 0..4 {
            if slot <= chunk.len() {
                let index = ((packed >> (18 - slot * 6)) & 0x3F) as usize;
                out.push(char::from(ALPHABET[index]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_standard_including_padding() {
        // The vectors from RFC 4648, because getting the padding wrong produces
        // a page that loads and then fails in the browser.
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn high_bytes_survive() {
        assert_eq!(base64(&[0x00, 0x10, 0x83, 0xFF]), "ABCD/w==");
    }
}
