//! `client_sizes`: whether a window is told the size it is placed at, and when.

use crate::script::ClientSizes;
use crate::script::doc::Doc;

/// Read the section. Each key left out keeps its default, and anything but
/// the words it takes is read as left out.
pub(crate) fn parse(options: &Doc) -> ClientSizes {
    let mut sizes = ClientSizes::default();
    if options.get("floating").text() == Some("ignore") {
        sizes.floating = false;
    }
    if let ignore @ Doc::Table(_) = options.get("ignore") {
        sizes.ignored = ignore
            .list()
            .iter()
            .filter_map(|app| app.text().map(str::to_owned))
            .collect();
    }
    sizes
}
