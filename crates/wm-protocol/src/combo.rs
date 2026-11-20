//! A key combination's one spelling: what a window manager binds and the
//! compositor matches a key against, so the two cannot disagree on whether
//! `shift+super+q` is `super+shift+q`.

/// Put a key combination into one canonical form.
///
/// So that `shift+super+q` and `super+shift+q` are the same binding. Done in
/// one place and used both when binding and when dispatching, because two
/// spellings of the same key is a bug that only shows up for whoever spelled it
/// the other way.
pub fn normalise(combo: &str) -> String {
    let mut modifiers = Vec::new();
    let mut key = String::new();

    for part in combo.split('+') {
        let part = part.trim().to_ascii_lowercase();
        match part.as_str() {
            "" => {}
            "ctrl" | "control" => modifiers.push("ctrl"),
            "alt" | "meta" => modifiers.push("alt"),
            "shift" => modifiers.push("shift"),
            "super" | "logo" | "win" | "cmd" => modifiers.push("super"),
            _ => key = part,
        }
    }

    // A fixed order, and each modifier once.
    let mut canonical = String::new();
    for name in ["ctrl", "alt", "shift", "super"] {
        if modifiers.contains(&name) {
            canonical.push_str(name);
            canonical.push('+');
        }
    }
    canonical.push_str(&key);
    canonical
}

#[cfg(test)]
mod tests {
    use super::normalise;

    #[test]
    fn a_combination_has_one_spelling() {
        assert_eq!(normalise("Shift+Super+Q"), "shift+super+q");
        assert_eq!(normalise("super+shift+q"), "shift+super+q");
        assert_eq!(normalise("logo+ctrl+alt+Delete"), "ctrl+alt+super+delete");
        assert_eq!(normalise("Return"), "return");
        assert_eq!(normalise("super+super+t"), "super+t");
    }
}
