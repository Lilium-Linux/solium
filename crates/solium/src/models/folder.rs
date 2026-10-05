//! The `Folder` model's rows, built from `Solium::folder` (04-ui.md §4.9).
//! Scanning itself -- the directory walk, MIME and icon resolution, trust --
//! is `crate::folder`; this is only the row shape a scene reads, the same
//! split `models::apps` keeps from `crate::apps`.

use std::collections::BTreeMap;

use super::diff::Row;
use crate::folder::Entry;
use crate::json::Json;

/// One row per entry of the desktop folder, already sorted by
/// [`crate::folder::scan`]. `tests::folder_rows_carry_every_field`.
pub(crate) fn rows(entries: &[Entry]) -> Vec<Row> {
    entries
        .iter()
        .map(|entry| Row {
            key: entry.uri.clone(),
            values: BTreeMap::from([
                ("uri", Json::Text(entry.uri.clone())),
                ("name", Json::Text(entry.name.clone())),
                ("displayName", Json::Text(entry.display_name.clone())),
                ("mime", Json::Text(entry.mime.clone())),
                ("icon", Json::Text(entry.icon.clone())),
                ("isDir", Json::Bool(entry.is_dir)),
                ("isLauncher", Json::Bool(entry.is_launcher)),
                ("trusted", Json::Bool(entry.trusted)),
                ("hidden", Json::Bool(entry.hidden)),
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a unix timestamp, far below 2^53"
                )]
                ("modified", Json::Number(entry.modified as f64)),
            ]),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> Entry {
        Entry {
            uri: format!("file:///home/x/Desktop/{name}"),
            name: name.to_owned(),
            display_name: name.to_owned(),
            mime: "application/pdf".to_owned(),
            icon: "application-pdf".to_owned(),
            is_dir: false,
            is_launcher: false,
            trusted: false,
            hidden: false,
            modified: 1_700_000_000,
        }
    }

    #[test]
    fn folder_rows_carry_every_field() {
        let entries = vec![entry("report.pdf")];
        let rows = rows(&entries);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, "file:///home/x/Desktop/report.pdf");
        assert_eq!(
            rows[0].values.get("displayName"),
            Some(&Json::Text("report.pdf".to_owned()))
        );
        assert_eq!(rows[0].values.get("isDir"), Some(&Json::Bool(false)));
        assert_eq!(
            rows[0].values.get("modified"),
            Some(&Json::Number(1_700_000_000.0))
        );
    }
}
