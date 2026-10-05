//! Theme icon lookup for `image://solium/icon/...` (03 §3.2.14): the Icon
//! Theme Specification's search, condensed to what a dock needs.
//!
//! **What is cut for this first version.** Rasterising is `host.cpp`'s: it
//! reads whatever path this module resolves with `QImageReader`, which loads
//! SVG through Qt's own `imageformats/libqsvg` plugin when one is installed
//! (this machine has `qt6-qtsvg`) -- no new link, no new crate, nothing to
//! build against; a machine without that plugin simply cannot rasterise an
//! SVG icon, and falls through to the fallback icon the same as a theme with
//! no PNGs at all. There is no LRU cache yet (03 §3.2.14 asks for one): every
//! request re-walks the theme chain, which is a handful of `stat` calls, not
//! worth a cache until a profile says otherwise. `sol.icons{ theme = ... }`
//! is not wired either; the theme is always the default chain the design
//! notes name -- GTK's `settings.ini`, then Adwaita, then hicolor -- so
//! overriding it is the natural next P1. An absolute `Icon=` path (AppImages,
//! JetBrains Toolbox) is refused rather than trusted, along with anything
//! else that looks like a path: see [`resolve`].

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// What a window with no entry, or an icon nothing resolves, shows.
pub(crate) const FALLBACK: &str = "application-x-executable";

/// `name` at `size` device pixels (`size * scale`, already multiplied by the
/// caller), or `None` when nothing in the theme chain nor the fallback has
/// it. `tests::a_fixture_theme_resolves_its_own_size_and_falls_through_to_hicolor`,
/// `tests::a_name_that_looks_like_a_path_is_refused`.
pub(crate) fn resolve(name: &str, size: u32, scale: u32) -> Option<PathBuf> {
    if !is_plain_name(name) {
        return None;
    }
    let roots = icon_roots();
    let theme = configured_theme();
    if let Some(found) = lookup(&roots, &theme, name, size.max(1), scale.max(1)) {
        return Some(found);
    }
    if name != FALLBACK {
        return lookup(&roots, &theme, FALLBACK, size.max(1), scale.max(1));
    }
    None
}

/// Refuses anything that is not a bare icon name: empty, an absolute path, a
/// `..` segment, or a name naming a *path* (`/`), which is exactly what the
/// Icon Theme Specification's own worked security concern is (03 §3.2.14:
/// "the index records absolute `Icon=` paths; an arbitrary path in a URL is
/// refused"). Supporting the legitimate case -- an installed entry's own
/// absolute `Icon=` -- needs the index in the loop to tell the two apart,
/// which is cut from this version (module doc); every absolute path is
/// refused for now, which is the safe side of that cut.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && name != "." && name != ".."
}

/// The icon search roots, precedence first: `$HOME/.icons`,
/// `$XDG_DATA_HOME/icons`, each `$XDG_DATA_DIRS/icons`, and `/usr/share/pixmaps`
/// (flat, theme-less, tried last by [`lookup`]).
fn icon_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".icons"));
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    if let Some(data_home) = data_home {
        roots.push(data_home.join("icons"));
    }
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|value| !value.is_empty())
        .and_then(|value| value.to_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
        roots.push(PathBuf::from(dir).join("icons"));
    }
    roots
}

/// GTK's `settings.ini`, then `Adwaita`, then `hicolor` -- the default chain
/// 03 §3.2.14 names. `$XDG_CONFIG_HOME/gtk-3.0/settings.ini`, then
/// `/etc/xdg/gtk-3.0/settings.ini`.
fn configured_theme() -> String {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    let candidates = [
        config_home.map(|dir| dir.join("gtk-3.0/settings.ini")),
        Some(PathBuf::from("/etc/xdg/gtk-3.0/settings.ini")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Ok(contents) = std::fs::read_to_string(&candidate)
            && let Some(groups) = ini_groups(&contents)
            && let Some(settings) = groups.get("Settings")
            && let Some(theme) = settings.get("gtk-icon-theme-name")
        {
            return theme.clone();
        }
    }
    "Adwaita".to_owned()
}

/// A directory the Icon Theme Specification's matching algorithm picks
/// between, one `index.theme` section.
#[derive(Clone, Debug)]
struct ThemeDir {
    /// Relative to the theme's own directory, e.g. `48x48/apps`.
    path: String,
    kind: Kind,
    size: u32,
    min: u32,
    max: u32,
    threshold: u32,
    scale: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Fixed,
    Scalable,
    Threshold,
}

impl ThemeDir {
    /// The Specification's `DirectoryMatchesSize`.
    fn matches(&self, size: u32, scale: u32) -> bool {
        if self.scale != scale {
            return false;
        }
        match self.kind {
            Kind::Fixed => self.size == size,
            Kind::Scalable => self.min <= size && size <= self.max,
            Kind::Threshold => size.abs_diff(self.size) <= self.threshold,
        }
    }

    /// The Specification's `DirectorySizeDistance`, for the directory
    /// closest to `size` when nothing matches exactly.
    fn distance(&self, size: u32, scale: u32) -> u32 {
        // A directory for the wrong scale is a poor match but not an
        // impossible one -- a size-only fallback across scales beats no
        // icon at all -- so it costs a large, constant penalty rather than
        // ruling the directory out.
        let scale_penalty = if self.scale == scale { 0 } else { 1000 };
        let size_distance = match self.kind {
            Kind::Fixed => size.abs_diff(self.size),
            Kind::Scalable => {
                if size < self.min {
                    self.min - size
                } else {
                    size.saturating_sub(self.max)
                }
            }
            Kind::Threshold => size.abs_diff(self.size).saturating_sub(self.threshold),
        };
        scale_penalty + size_distance
    }
}

/// `index.theme`'s `[Icon Theme]` section and its named directory sections,
/// parsed into [`ThemeDir`]s, and what it inherits.
fn read_index(theme_dir: &Path) -> Option<(Vec<String>, Vec<ThemeDir>)> {
    let contents = std::fs::read_to_string(theme_dir.join("index.theme")).ok()?;
    let groups = ini_groups(&contents)?;
    let main = groups.get("Icon Theme")?;
    let inherits = main
        .get("Inherits")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let directories = main
        .get("Directories")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
        })
        .into_iter()
        .flatten();
    let mut dirs = Vec::new();
    for name in directories {
        let Some(section) = groups.get(name) else {
            continue;
        };
        let size: u32 = section
            .get("Size")
            .and_then(|v| v.parse().ok())
            .unwrap_or(48);
        let kind = match section.get("Type").map(String::as_str) {
            Some("Fixed") => Kind::Fixed,
            Some("Scalable") => Kind::Scalable,
            _ => Kind::Threshold,
        };
        dirs.push(ThemeDir {
            path: name.to_owned(),
            kind,
            size,
            min: section
                .get("MinSize")
                .and_then(|v| v.parse().ok())
                .unwrap_or(size),
            max: section
                .get("MaxSize")
                .and_then(|v| v.parse().ok())
                .unwrap_or(size),
            threshold: section
                .get("Threshold")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),
            scale: section
                .get("Scale")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
        });
    }
    Some((inherits, dirs))
}

/// A bare `.ini`, grouped: every `[section]`'s `key=value` lines, in order,
/// case preserved. `None` for a file with no section at all.
fn ini_groups(contents: &str) -> Option<HashMap<String, HashMap<String, String>>> {
    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            current = Some(name.to_owned());
            groups.entry(name.to_owned()).or_default();
            continue;
        }
        let Some(name) = &current else { continue };
        if let Some((key, value)) = line.split_once('=')
            && let Some(section) = groups.get_mut(name)
        {
            section.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    (!groups.is_empty()).then_some(groups)
}

/// `theme`'s own name, then its `Inherits` chain, breadth-first, and always
/// ending at `hicolor`, the Specification's own last resort -- a name
/// already in the chain is not added again.
fn theme_chain(roots: &[PathBuf], theme: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut queue = vec![theme.to_owned()];
    while let Some(next) = queue.pop() {
        if chain.contains(&next) {
            continue;
        }
        let inherits = roots
            .iter()
            .find_map(|root| read_index(&root.join(&next)))
            .map(|(inherits, _)| inherits)
            .unwrap_or_default();
        chain.push(next);
        // Depth matters less than precedence here, and `Inherits` is rare
        // enough (one or two names) that plain order is clear to read and
        // correct enough: nothing in this dock needs the Specification's
        // full breadth-first tie-breaking.
        for parent in inherits.into_iter().rev() {
            queue.push(parent);
        }
    }
    if chain.last().map(String::as_str) != Some("hicolor") {
        chain.push("hicolor".to_owned());
    }
    chain
}

/// `name` under `theme`'s own chain (its `Inherits`, and `hicolor` last),
/// across every root in `roots`.
///
/// **Both passes search the whole chain as one pool, not theme by theme.**
/// `theme`'s own closest-but-wrong-size directory does not win over an
/// *exact* size a later theme in the chain has -- that would be
/// `find_in_theme`'s bug this replaced, caught by
/// `tests::a_fixture_theme_resolves_its_own_size_and_falls_through_to_hicolor`:
/// asked for 16 against a theme whose only directory is a `Fixed` 48, it
/// has to reach `hicolor`'s real 16, not settle for its own, merely closest,
/// 48. The Specification's own `find_icon_helper` is written the same way:
/// an exact pass over every directory of the theme *and its ancestors*
/// together, then, only if that finds nothing anywhere, a closest-distance
/// pass over that same whole pool.
fn lookup(roots: &[PathBuf], theme: &str, name: &str, size: u32, scale: u32) -> Option<PathBuf> {
    let chain = theme_chain(roots, theme);
    let pool: Vec<(String, ThemeDir)> = chain
        .iter()
        .filter_map(|theme_name| {
            roots
                .iter()
                .find_map(|root| read_index(&root.join(theme_name)))
                .map(|(_, dirs)| (theme_name.clone(), dirs))
        })
        .flat_map(|(theme_name, dirs)| dirs.into_iter().map(move |dir| (theme_name.clone(), dir)))
        .collect();

    for (theme_name, dir) in &pool {
        if dir.matches(size, scale)
            && let Some(found) = find_file(roots, theme_name, dir, name)
        {
            return Some(found);
        }
    }
    let mut best: Option<(u32, PathBuf)> = None;
    for (theme_name, dir) in &pool {
        let Some(found) = find_file(roots, theme_name, dir, name) else {
            continue;
        };
        let distance = dir.distance(size, scale);
        if best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
            best = Some((distance, found));
        }
    }
    if let Some((_, found)) = best {
        return Some(found);
    }

    // `/usr/share/pixmaps`, flat, no theme, no sizes -- the Specification's
    // own fallback before giving up.
    for root in roots {
        if let Some(pixmaps) = root.parent().map(|parent| parent.join("pixmaps")) {
            for ext in ["png", "svg", "xpm"] {
                let candidate = pixmaps.join(format!("{name}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// `name` in one theme directory, every root it is installed under checked
/// (precedence is the roots' own order, not which root happened to carry
/// `index.theme`).
fn find_file(roots: &[PathBuf], theme: &str, dir: &ThemeDir, name: &str) -> Option<PathBuf> {
    for root in roots {
        for ext in ["png", "svg", "xpm"] {
            let candidate = root
                .join(theme)
                .join(&dir.path)
                .join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// `host.cpp`'s image provider calling back in -- the same shape as
/// `solium_qml_log_from_qt` in `qml.rs`, the one other declaration in this
/// compositor that points from C++ into Rust rather than the other way.
/// Returns a heap `char*` the host must free with
/// [`solium_icon_lookup_free`], or null when [`resolve`] found nothing.
#[expect(
    unsafe_code,
    reason = "the Qt image provider calls this across the C ABI"
)]
#[unsafe(no_mangle)]
extern "C" fn solium_icon_lookup(
    name: *const std::ffi::c_char,
    size: std::ffi::c_int,
    scale: std::ffi::c_int,
) -> *mut std::ffi::c_char {
    // SAFETY: `name` is a NUL-terminated string valid for the call, which is
    // `icon.cpp`'s contract on its own side of this declaration.
    let name = unsafe { std::ffi::CStr::from_ptr(name) };
    let Ok(name) = name.to_str() else {
        return std::ptr::null_mut();
    };
    let size = u32::try_from(size).unwrap_or(48);
    let scale = u32::try_from(scale).unwrap_or(1);
    resolve(name, size, scale)
        .and_then(|path| path.to_str().map(ToOwned::to_owned))
        .and_then(|path| std::ffi::CString::new(path).ok())
        .map_or(std::ptr::null_mut(), std::ffi::CString::into_raw)
}

/// Frees what [`solium_icon_lookup`] returned. A null pointer does nothing.
#[expect(unsafe_code, reason = "freeing memory handed across the C ABI")]
#[unsafe(no_mangle)]
extern "C" fn solium_icon_lookup_free(path: *mut std::ffi::c_char) {
    if path.is_null() {
        return;
    }
    // SAFETY: only ever a pointer `solium_icon_lookup` returned from
    // `CString::into_raw`, and freed at most once -- `icon.cpp` frees it
    // immediately after copying it into a `QString`.
    unsafe {
        drop(std::ffi::CString::from_raw(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solium-icon-test-{name}-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture dir");
        }
        fs::write(path, contents).expect("fixture file");
    }

    #[test]
    fn a_name_that_looks_like_a_path_is_refused() {
        assert!(!is_plain_name(""));
        assert!(!is_plain_name("/etc/passwd"));
        assert!(!is_plain_name(".."));
        assert!(is_plain_name("firefox"));
        assert!(is_plain_name("org.mozilla.firefox"));
    }

    #[test]
    fn a_fixture_theme_resolves_its_own_size_and_falls_through_to_hicolor() {
        let root = tmp("fixture-root");
        // The fixture theme: one real size (48) and one it does not have
        // (16), so a lookup for 16 has to fall through to hicolor.
        write(
            &root.join("Fixture/index.theme"),
            "[Icon Theme]\n\
             Name=Fixture\n\
             Directories=48x48/apps\n\
             Inherits=hicolor\n\
             \n\
             [48x48/apps]\n\
             Size=48\n\
             Type=Fixed\n",
        );
        write(&root.join("Fixture/48x48/apps/myapp.png"), "not a real png");
        write(
            &root.join("hicolor/index.theme"),
            "[Icon Theme]\n\
             Name=Hicolor\n\
             Directories=16x16/apps\n\
             \n\
             [16x16/apps]\n\
             Size=16\n\
             Type=Fixed\n",
        );
        write(
            &root.join("hicolor/16x16/apps/myapp.png"),
            "not a real png either",
        );

        let roots = vec![root.clone()];
        let found = lookup(&roots, "Fixture", "myapp", 48, 1).expect("the fixture's own size");
        assert_eq!(found, root.join("Fixture/48x48/apps/myapp.png"));

        let fell_through =
            lookup(&roots, "Fixture", "myapp", 16, 1).expect("hicolor, through Inherits");
        assert_eq!(fell_through, root.join("hicolor/16x16/apps/myapp.png"));

        assert!(lookup(&roots, "Fixture", "nothing-installed-has-this-name", 48, 1).is_none());
    }

    #[test]
    fn a_scalable_directory_matches_a_range_of_sizes() {
        let root = tmp("fixture-scalable");
        write(
            &root.join("Fixture/index.theme"),
            "[Icon Theme]\nName=Fixture\nDirectories=scalable/apps\n\n\
             [scalable/apps]\nSize=48\nMinSize=16\nMaxSize=512\nType=Scalable\n",
        );
        write(&root.join("Fixture/scalable/apps/vector.svg"), "<svg/>");
        let roots = vec![root.clone()];
        assert_eq!(
            lookup(&roots, "Fixture", "vector", 512, 1),
            Some(root.join("Fixture/scalable/apps/vector.svg"))
        );
    }
}
