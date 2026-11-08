//! The production source of `script/`, for the tests that read it as text:
//! the dispatch vocabulary, the reference, and where `mlua` may appear.
//!
//! One list, checked against the directory by
//! `every_file_of_the_module_is_listed`, so a file added to the module cannot
//! be missed by a scan that would then agree with anything.

/// Every production file of `script/`, by its path under `src/script/`.
pub(crate) const FILES: &[(&str, &str)] = &[
    ("command.rs", include_str!("command.rs")),
    ("doc.rs", include_str!("doc.rs")),
    ("keep.rs", include_str!("keep.rs")),
    ("lua/convert.rs", include_str!("lua/convert.rs")),
    ("lua/api/bindings.rs", include_str!("lua/api/bindings.rs")),
    ("lua/api/input.rs", include_str!("lua/api/input.rs")),
    ("lua/api/keep.rs", include_str!("lua/api/keep.rs")),
    ("lua/api/layout.rs", include_str!("lua/api/layout.rs")),
    ("lua/api/mod.rs", include_str!("lua/api/mod.rs")),
    ("lua/api/monitors.rs", include_str!("lua/api/monitors.rs")),
    ("lua/api/present.rs", include_str!("lua/api/present.rs")),
    ("lua/api/process.rs", include_str!("lua/api/process.rs")),
    ("lua/api/services.rs", include_str!("lua/api/services.rs")),
    ("lua/api/settings.rs", include_str!("lua/api/settings.rs")),
    ("lua/api/windows.rs", include_str!("lua/api/windows.rs")),
    ("lua/bindings.rs", include_str!("lua/bindings.rs")),
    ("lua/deadline.rs", include_str!("lua/deadline.rs")),
    ("lua/files.rs", include_str!("lua/files.rs")),
    ("lua/layouts.rs", include_str!("lua/layouts.rs")),
    ("lua/motion.rs", include_str!("lua/motion.rs")),
    ("lua/pending.rs", include_str!("lua/pending.rs")),
    ("lua/runs.rs", include_str!("lua/runs.rs")),
    ("lua/dispatch.rs", include_str!("lua/dispatch.rs")),
    ("lua/load.rs", include_str!("lua/load.rs")),
    ("lua/mod.rs", include_str!("lua/mod.rs")),
    ("mod.rs", include_str!("mod.rs")),
    ("reference.rs", include_str!("reference.rs")),
    ("settings/focus.rs", include_str!("settings/focus.rs")),
    ("settings/frames.rs", include_str!("settings/frames.rs")),
    ("settings/idle.rs", include_str!("settings/idle.rs")),
    ("settings/input.rs", include_str!("settings/input.rs")),
    ("settings/lock.rs", include_str!("settings/lock.rs")),
    ("settings/reload.rs", include_str!("settings/reload.rs")),
    (
        "settings/layer_shell.rs",
        include_str!("settings/layer_shell.rs"),
    ),
    (
        "settings/toplevels.rs",
        include_str!("settings/toplevels.rs"),
    ),
    ("settings/x11.rs", include_str!("settings/x11.rs")),
    ("settings/session.rs", include_str!("settings/session.rs")),
    ("settings/run.rs", include_str!("settings/run.rs")),
    ("settings/files.rs", include_str!("settings/files.rs")),
    (
        "settings/client_sizes.rs",
        include_str!("settings/client_sizes.rs"),
    ),
    ("settings/resize.rs", include_str!("settings/resize.rs")),
    ("settings/cursor.rs", include_str!("settings/cursor.rs")),
    ("settings/qml.rs", include_str!("settings/qml.rs")),
    ("settings/pane.rs", include_str!("settings/pane.rs")),
    (
        "settings/fullscreen.rs",
        include_str!("settings/fullscreen.rs"),
    ),
    ("settings/services.rs", include_str!("settings/services.rs")),
    ("settings/keyboard.rs", include_str!("settings/keyboard.rs")),
    ("settings/monitors.rs", include_str!("settings/monitors.rs")),
    ("settings/effects.rs", include_str!("settings/effects.rs")),
    ("settings/mod.rs", include_str!("settings/mod.rs")),
    ("settings/read.rs", include_str!("settings/read.rs")),
    (
        "settings/workspaces.rs",
        include_str!("settings/workspaces.rs"),
    ),
    ("source.rs", include_str!("source.rs")),
    ("types.rs", include_str!("types.rs")),
];

/// Each file cut at its first `#[cfg(test)]`, joined: the code a normal
/// build compiles, and none of the tests that talk about it.
pub(crate) fn production() -> &'static str {
    static JOINED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    JOINED.get_or_init(|| {
        FILES
            .iter()
            .map(|(_, text)| text.split("#[cfg(test)]").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::FILES;

    /// Every `.rs` under `src/script/`, outside `tests/`, is in [`FILES`].
    #[test]
    fn every_file_of_the_module_is_listed() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/script");
        let mut found = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src/script reads") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "tests") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let relative = path.strip_prefix(&root).expect("under the root");
                    found.push(relative.to_string_lossy().into_owned());
                }
            }
        }
        found.sort();
        let mut listed: Vec<String> = FILES.iter().map(|(path, _)| (*path).to_owned()).collect();
        listed.sort();
        assert_eq!(found, listed, "src/script/ and source::FILES disagree");
    }

    /// `mlua` only in the Lua layer, and in the scans that look for it: the
    /// rest of the module is what a window manager in any language talks to.
    #[test]
    fn mlua_appears_only_under_lua() {
        let outside: Vec<&str> = FILES
            .iter()
            .filter(|(path, _)| {
                !path.starts_with("lua/") && *path != "reference.rs" && *path != "source.rs"
            })
            .filter(|(_, text)| {
                text.split("#[cfg(test)]")
                    .next()
                    .unwrap_or_default()
                    .contains("mlua")
            })
            .map(|(path, _)| *path)
            .collect();
        assert!(outside.is_empty(), "mlua outside script/lua/: {outside:?}");
    }
}
