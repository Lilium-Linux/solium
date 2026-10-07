//! The `Apps` model's rows, built from `Solium::apps` (03 §3.2.13). Scanning
//! itself -- the XDG walk, parsing, precedence -- is `crate::apps`; this is
//! only the row shape a scene reads.

use std::collections::BTreeMap;

use super::diff::Row;
use crate::apps::Entry;
use crate::json::Json;

/// One row per installed, visible application, sorted by name (already the
/// order [`crate::apps::scan`] leaves them in). Takes the entries rather than
/// `&Solium` so this is testable without a compositor to build one, the same
/// reason `solium-layout` and `solium-animation` are their own crates.
/// `tests::apps_rows_carry_name_icon_and_categories`.
pub(crate) fn rows(apps: &[Entry]) -> Vec<Row> {
    apps.iter()
        .map(|entry| Row {
            key: entry.id.clone(),
            values: BTreeMap::from([
                ("name", Json::Text(entry.name.clone())),
                ("genericName", Json::Text(entry.generic_name.clone())),
                ("icon", Json::Text(entry.icon.clone())),
                (
                    "categories",
                    Json::List(entry.categories.iter().cloned().map(Json::Text).collect()),
                ),
                (
                    "keywords",
                    Json::List(entry.keywords.iter().cloned().map(Json::Text).collect()),
                ),
            ]),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::Entry;
    use std::path::PathBuf;

    fn entry(id: &str, name: &str) -> Entry {
        Entry {
            id: id.to_owned(),
            name: name.to_owned(),
            untranslated_name: name.to_owned(),
            generic_name: "Web Browser".to_owned(),
            icon: "firefox".to_owned(),
            exec: "firefox %u".to_owned(),
            categories: vec!["Network".to_owned(), "WebBrowser".to_owned()],
            keywords: Vec::new(),
            terminal: false,
            path: None,
            source: PathBuf::from("/usr/share/applications/firefox.desktop"),
        }
    }

    #[test]
    fn apps_rows_carry_name_icon_and_categories() {
        let apps = vec![entry("org.mozilla.firefox", "Firefox")];
        let rows = rows(&apps);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, "org.mozilla.firefox");
        assert_eq!(
            rows[0].values.get("name"),
            Some(&Json::Text("Firefox".to_owned()))
        );
        assert_eq!(
            rows[0].values.get("categories"),
            Some(&Json::List(vec![
                Json::Text("Network".to_owned()),
                Json::Text("WebBrowser".to_owned())
            ]))
        );
    }
}
