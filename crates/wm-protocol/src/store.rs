//! What may name a store: the rule both peers hold to.

/// Whether `name` may name a store: letters, digits, `_` and `-`, so it is
/// always one file in the compositor's state directory.
pub fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}
