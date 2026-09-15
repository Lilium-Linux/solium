//! Build the effects preview page.
//!
//! Embeds the deformation engine — compiled to WebAssembly — into
//! `preview/index.html`, and writes the result. The page calls it directly, so
//! the mesh you watch bend in a browser is the one `warp::mesh` builds for a
//! real window. A reimplementation in JavaScript would drift the first time
//! either side changed, and the drift would be invisible because the page would
//! still bend something plausible.
//!
//! Driven by `dev/preview`, which builds the wasm first.
//!
//! ```sh
//! dev/preview && xdg-open crates/effects/preview/preview.html
//! ```
//!
//! The binary is `effects-preview` rather than `preview`; `Cargo.toml` says
//! why.

use std::{io::Write as _, path::PathBuf};

use solium_effects::{Axis, Deform};

fn main() {
    let Some(wasm) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: effects-preview <effects.wasm>");
        eprintln!("       run dev/preview instead, which builds it first");
        std::process::exit(2);
    };

    let bytes = match std::fs::read(&wasm) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("error: could not read {}: {err}", wasm.display());
            std::process::exit(1);
        }
    };

    // Both lists come from the engine, in the order its `all()` gives them,
    // which is the order the exported functions index by. A list written out
    // here would be a second copy of a vocabulary, and the page would select
    // the wrong effect the first time one was inserted rather than appended.
    let names = |list: Vec<&str>| {
        list.into_iter()
            .map(|name| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let effects = names(Deform::all().iter().map(|(name, _)| *name).collect());
    let axes = names(Axis::all().iter().map(|(name, _)| *name).collect());

    const PAGE: &str = include_str!("../../preview/index.html");
    let page = PAGE
        .replace("/*EFFECTS*/ null", &format!("[{effects}]"))
        .replace("/*AXES*/ null", &format!("[{axes}]"))
        .replace("/*WASM*/ \"\"", &format!("{:?}", base64(&bytes)));

    let mut stdout = std::io::stdout().lock();
    if let Err(err) = stdout.write_all(page.as_bytes()) {
        eprintln!("error: could not write the page: {err}");
        std::process::exit(1);
    }
}

/// Base64, so the wasm can live inside the page.
///
/// The same twenty lines as `solium-animation`'s preview builder, and a
/// deliberate copy rather than a shared helper: sharing it would mean this
/// crate depending on that one, and the empty `[dependencies]` is what keeps
/// an effect testable without a compositor. Twenty lines with RFC 4648's own
/// vectors under them is the cheaper half of that trade.
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

    /// Every placeholder the page carries is one this binary fills in.
    ///
    /// A renamed marker is otherwise a page that loads, shows an empty effect
    /// list and blames the engine.
    #[test]
    fn the_page_has_somewhere_to_put_everything() {
        const PAGE: &str = include_str!("../../preview/index.html");
        for marker in ["/*EFFECTS*/ null", "/*AXES*/ null", "/*WASM*/ \"\""] {
            assert!(PAGE.contains(marker), "the page has no {marker} to fill in");
        }
    }
}
