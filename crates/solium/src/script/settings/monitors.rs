//! `monitors`: where each screen goes, at what mode, scale and rotation.

use smithay::utils::{Logical, Point};

use super::read;
use crate::script::doc::Doc;

/// Read the list of rows. A row of the wrong kind, or a number of the wrong
/// kind in one, refuses the list in the words Lua's conversions use; a row
/// with no name is named in the log and left out.
pub(crate) fn parse(rows: &Doc) -> Result<Vec<crate::monitor::Placement>, String> {
    let mut places = Vec::new();
    for row in rows.list() {
        if !matches!(row, Doc::Table(_)) {
            return Err(read::not_a(row, "table"));
        }
        // A row with no name cannot be matched to a connector, and
        // silently dropping it is how a configuration appears to be
        // ignored. Say which row, because the list has no other
        // landmarks.
        let Doc::Text(name) = row.get("name") else {
            tracing::warn!(
                row = places.len() + 1,
                "a monitor with no name -- run `solium --probe` for the ones this \
                 machine has"
            );
            continue;
        };
        let name = name.clone();

        // A position outright, when both halves were given. One half
        // alone is treated as unset rather than as zero: `y = 200` on
        // its own almost certainly means "and leave x alone", and
        // reading the other as 0 would slide the monitor to the left
        // edge of the desk for no stated reason.
        let (x, y) = (
            read::whole::<i32>(row, "x", "i32")?,
            read::whole::<i32>(row, "y", "i32")?,
        );
        let at = match (x, y) {
            (Some(x), Some(y)) => Some(Point::<i32, Logical>::from((x, y))),
            (Some(x), None) => Some(Point::from((x, 0))),
            (None, Some(y)) => Some(Point::from((0, y))),
            (None, None) => None,
        };

        // Or beside another monitor, which is the form that does not
        // go stale when a resolution changes.
        let align = match row.get("align") {
            Doc::Text(name) => crate::monitor::align(name).unwrap_or_default(),
            _ => crate::monitor::Align::default(),
        };
        let mut beside = None;
        for (key, side) in [
            ("right_of", crate::monitor::Side::Right),
            ("left_of", crate::monitor::Side::Left),
            ("above", crate::monitor::Side::Above),
            ("below", crate::monitor::Side::Below),
        ] {
            if let Doc::Text(anchor) = row.get(key) {
                beside = Some((side, anchor.clone(), align));
                break;
            }
        }

        // A mode, written the way every display tool writes one:
        // `"2560x1440@165"`. A table with `w`, `h` and `refresh` does
        // the same thing, for anyone generating a configuration rather
        // than typing it.
        let mode = match row.get("mode") {
            Doc::Text(text) => {
                let text = Some(text.clone());
                match text.as_deref().and_then(crate::monitor::mode) {
                    Some(mode) => mode,
                    None => {
                        tracing::warn!(
                            monitor = name,
                            mode = ?text,
                            "not a mode -- write it as 2560x1440@165, or as one of \
                             best, preferred, widest"
                        );
                        crate::monitor::Wanted::default()
                    }
                }
            }
            mode @ Doc::Table(_) => {
                match (
                    read::whole::<i32>(mode, "w", "i32")?,
                    read::whole::<i32>(mode, "h", "i32")?,
                ) {
                    (Some(width), Some(height)) if width > 0 && height > 0 => {
                        crate::monitor::Wanted::Exact {
                            width,
                            height,
                            refresh: read::whole::<i32>(mode, "refresh", "i32")?,
                        }
                    }
                    _ => {
                        tracing::warn!(
                            monitor = name,
                            "a mode table needs both w and h -- ignoring it"
                        );
                        crate::monitor::Wanted::default()
                    }
                }
            }
            _ => crate::monitor::Wanted::default(),
        };

        let transform = match row.get("transform") {
            Doc::Text(value) => {
                let value = Some(value.clone());
                match value.as_deref().and_then(crate::monitor::transform) {
                    Some(transform) => Some(transform),
                    None => {
                        tracing::warn!(
                            monitor = name,
                            transform = ?value,
                            "not a transform -- one of normal, 90, 180, 270, \
                             flipped, flipped-90, flipped-180, flipped-270"
                        );
                        None
                    }
                }
            }
            // A number is what people write for a rotation, and
            // refusing it over a quotation mark would be pedantry.
            Doc::Int(degrees) => crate::monitor::transform(&degrees.to_string()),
            _ => None,
        };

        let name_for_warning = name.clone();
        places.push(crate::monitor::Placement {
            name,
            at,
            beside,
            mode,
            vrr: read::flag(row, "vrr"),
            transform,
            enabled: read::flag(row, "enabled").unwrap_or(true),
            primary: read::flag(row, "primary").unwrap_or(false),
            scale: match row.get("scale") {
                Doc::Num(value) => match crate::monitor::scaling(value) {
                    Some(scale) => scale,
                    None => {
                        tracing::warn!(
                            monitor = name_for_warning,
                            scale = value,
                            "not a sensible scale -- between 0.5 and 8, or \"auto\""
                        );
                        crate::monitor::Scaling::Auto
                    }
                },
                Doc::Int(value) => {
                    #[expect(clippy::cast_precision_loss, reason = "a scale is a small number")]
                    let value = *value as f64;
                    crate::monitor::scaling(&value).unwrap_or(crate::monitor::Scaling::Auto)
                }
                // "auto", and anything unreadable, which means the
                // same thing and has already been warned about.
                _ => crate::monitor::Scaling::Auto,
            },
        });
    }
    Ok(places)
}
