//! `lock`: the locker logind's `Lock` runs, and Solium's own lock: how it
//! checks a password or a PIN, and what it shows.

use crate::script::doc::{Doc, describe};

/// Read the section: what logind's signals do, how the own lock checks a
/// secret, and what it shows. Each key left out keeps its default, and one
/// of the wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(
    options: &Doc,
) -> (
    crate::logind::Settings,
    crate::lock_auth::Settings,
    crate::lock_scene::Settings,
) {
    let mut settings = crate::logind::Settings::default();
    let auth = auth(options);
    match options.get("command") {
        Doc::Text(command) => settings.command = Some(command.clone()),
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "lock.command is a string, like \"swaylock -f\"; keeping it unset"
        ),
    }
    match options.get("before_sleep") {
        Doc::Bool(on) => settings.before_sleep = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "lock.before_sleep is true or false; keeping the default, true"
        ),
    }
    match options.get("logind_unlock") {
        Doc::Bool(on) => settings.logind_unlock = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "lock.logind_unlock is true or false; keeping the default, true"
        ),
    }
    let scene = scene(options, settings.command.is_some());
    (settings, auth, scene)
}

/// `sol.lock`'s settings for Solium's own lock (#281): `lockout` (a table of
/// `attempts`, `seconds` and `max_seconds`, whole numbers of at least 1),
/// `secret_layout` (`"first"` or `"current"`), `helper_timeout_ms` (a whole
/// number of at least 100), `retry` (`seconds` and `max_seconds`, and
/// `attempts`, a whole number of at least 0) and
/// `keypad` (`"digits"`, `"letters"` or `"keyboard"`). Each one absent keeps
/// its default; each one of the wrong kind is named in the log and keeps its
/// default. A `max_seconds` under its `seconds` is taken as `seconds`.
/// `tests::sol_lock_parses_the_native_locks_settings`.
fn auth(options: &Doc) -> crate::lock_auth::Settings {
    let mut settings = crate::lock_auth::Settings::default();
    let whole = |value: &Doc, least: u32| -> Option<u32> {
        let number = match value {
            Doc::Int(number) => *number,
            Doc::Num(number) if number.fract() == 0.0 && number.is_finite() => {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a whole number, bounded by the conversion below"
                )]
                let whole = *number as i64;
                whole
            }
            _ => return None,
        };
        u32::try_from(number).ok().filter(|number| *number >= least)
    };
    let backoff = |table: &Doc,
                   name: &str,
                   mut wait: crate::lock_auth::Backoff|
     -> crate::lock_auth::Backoff {
        for (key, slot) in [
            ("seconds", &mut wait.seconds),
            ("max_seconds", &mut wait.max_seconds),
        ] {
            match table.get(key) {
                Doc::Nil => {}
                value => match whole(value, 1) {
                    Some(number) => *slot = number,
                    None => tracing::warn!(
                        value = describe(value),
                        "lock.{name}.{key} is a whole number of seconds, 1 or more; keeping the default"
                    ),
                },
            }
        }
        wait.max_seconds = wait.max_seconds.max(wait.seconds);
        wait
    };
    match options.get("lockout") {
        table @ Doc::Table(_) => {
            settings.lockout.wait = backoff(table, "lockout", settings.lockout.wait);
            match table.get("attempts") {
                Doc::Nil => {}
                value => match whole(value, 1) {
                    Some(attempts) => settings.lockout.attempts = attempts,
                    None => tracing::warn!(
                        value = describe(value),
                        "lock.lockout.attempts is a whole number, 1 or more; keeping the default, 3"
                    ),
                },
            }
        }
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "lock.lockout is a table, like {{ attempts = 3, seconds = 30, max_seconds = 300 }}; keeping the default"
        ),
    }
    match options.get("retry") {
        table @ Doc::Table(_) => {
            settings.retry = backoff(table, "retry", settings.retry);
            match table.get("attempts") {
                Doc::Nil => {}
                value => match whole(value, 0) {
                    Some(attempts) => settings.retries = attempts,
                    None => tracing::warn!(
                        value = describe(value),
                        "lock.retry.attempts is a whole number, 0 or more; keeping the default, 5"
                    ),
                },
            }
        }
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "lock.retry is a table, like {{ seconds = 2, max_seconds = 60, attempts = 5 }}; keeping the default"
        ),
    }
    match options.get("secret_layout") {
        Doc::Nil => {}
        value => match value
            .text()
            .map(str::to_owned)
            .and_then(|name| crate::lock_auth::SecretLayout::named(&name))
        {
            Some(layout) => settings.secret_layout = layout,
            None => tracing::warn!(
                value = describe(value),
                "lock.secret_layout is \"first\" or \"current\"; keeping the default, \"first\""
            ),
        },
    }
    match options.get("helper_timeout_ms") {
        Doc::Nil => {}
        value => match whole(value, 100) {
            Some(ms) => settings.helper_timeout = std::time::Duration::from_millis(u64::from(ms)),
            None => tracing::warn!(
                value = describe(value),
                "lock.helper_timeout_ms is a whole number of milliseconds, 100 or more; keeping the default, 30000"
            ),
        },
    }
    match options.get("keypad") {
        Doc::Nil => {}
        value => match value
            .text()
            .map(str::to_owned)
            .and_then(|name| crate::lock_auth::Keypad::named(&name))
        {
            Some(keypad) => settings.keypad = keypad,
            None => tracing::warn!(
                value = describe(value),
                "lock.keypad is \"digits\", \"letters\" or \"keyboard\"; keeping the default, \"digits\""
            ),
        },
    }
    match options.get("pin") {
        table @ Doc::Table(_) => settings.pin = pin(table, &whole),
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "lock.pin is a table, like {{ enabled = true, min_length = 4, max_length = 8, attempts = 5, password_after_hours = 48 }}; keeping the default"
        ),
    }
    settings
}

/// `lock.pin` (#281): `enabled` (true or false), `min_length` (a whole number
/// of digits, at least `pin::MIN_LENGTH`), `max_length` (at least
/// `min_length`, at most `pin::MAX_LENGTH`), `attempts` (1 to
/// `pin::MAX_ATTEMPTS`) and `password_after_hours` (1 to
/// `pin::MAX_PASSWORD_AFTER_HOURS`), a number past either cap taken as the
/// cap, with the log saying so. Each one absent keeps its default;
/// each one of the wrong kind is named in the log and keeps its default. A
/// `max_length` under `min_length` is taken as `min_length`.
/// `tests::sol_lock_parses_the_pins_settings`.
fn pin(table: &Doc, whole: &dyn Fn(&Doc, u32) -> Option<u32>) -> crate::pin::Settings {
    use crate::pin::{MAX_ATTEMPTS, MAX_LENGTH, MAX_PASSWORD_AFTER_HOURS, MIN_LENGTH, Settings};
    let mut settings = Settings::default();
    match table.get("enabled") {
        Doc::Bool(on) => settings.enabled = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "lock.pin.enabled is true or false; keeping the default, true"
        ),
    }
    let number = |key: &str, least: u32, most: u32, default: u32| -> Option<u32> {
        match table.get(key) {
            Doc::Nil => None,
            value => match whole(value, least).filter(|number| *number <= most) {
                Some(number) => Some(number),
                None => {
                    tracing::warn!(
                        value = describe(value),
                        "lock.pin.{key} is a whole number from {least} to {most}; keeping the default, {default}"
                    );
                    None
                }
            },
        }
    };
    let min = u32::try_from(MIN_LENGTH).unwrap_or(4);
    let max = u32::try_from(MAX_LENGTH).unwrap_or(64);
    if let Some(length) = number("min_length", min, max, 4) {
        settings.min_length = usize::try_from(length).unwrap_or(MIN_LENGTH);
    }
    if let Some(length) = number("max_length", min, max, 8) {
        settings.max_length = usize::try_from(length).unwrap_or(MAX_LENGTH);
    }
    settings.max_length = settings.max_length.max(settings.min_length);
    // These two only ever loosen the PIN, so they are capped: a number past
    // the cap is taken as the cap, and the log says so.
    let capped = |key: &str, cap: u32, default: u32| -> Option<u32> {
        let number = number(key, 1, u32::MAX, default)?;
        if number > cap {
            tracing::warn!(
                value = number,
                "lock.pin.{key} is at most {cap}; taking {cap}"
            );
        }
        Some(number.min(cap))
    };
    if let Some(attempts) = capped("attempts", MAX_ATTEMPTS, 5) {
        settings.attempts = attempts;
    }
    if let Some(hours) = capped("password_after_hours", MAX_PASSWORD_AFTER_HOURS, 48) {
        settings.password_after = std::time::Duration::from_secs(u64::from(hours) * 3600);
    }
    settings
}

/// `sol.lock`'s settings for what Solium's own lock shows (#281): `native`
/// (true or false; unset, true unless `command` names a locker, as
/// `has_command` says), `scene` (a path or a name, not empty), `prompt_on`
/// (`"primary"`, `"pointer"` or a monitor's name), `enter_ms` and `exit_ms`
/// (whole numbers of milliseconds, 0 for none, at most
/// `lock_scene::ENTER_MAX_MS` and `lock_scene::EXIT_MAX_MS`, a longer one
/// taken as that) and `keypad_margin` (a whole number of pixels, 0 or more).
/// Each one absent keeps its default; each one of the wrong kind is named in
/// the log and keeps its default.
/// `tests::sol_lock_parses_what_the_native_lock_shows`.
/// `lock.<key>`, a whole number of milliseconds from 0 to `max`: `None` when
/// it is absent, or of the wrong kind, which the log names with `default`
/// kept; one past `max` is taken as `max`, and the log says `why`.
fn bounded_ms(
    options: &Doc,
    key: &str,
    default: u32,
    max: u32,
    why: &str,
) -> Option<std::time::Duration> {
    let value = match options.get(key) {
        Doc::Nil => return None,
        value => value,
    };
    let whole = match &value {
        Doc::Int(number) => Some(*number),
        Doc::Num(number) if number.fract() == 0.0 && number.is_finite() => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a whole number, bounded below"
            )]
            let whole = *number as i64;
            Some(whole)
        }
        _ => None,
    };
    match whole.and_then(|ms| u64::try_from(ms).ok()) {
        Some(ms) if ms > u64::from(max) => {
            tracing::warn!(ms, "lock.{key} is at most {max}: {why}; taking {max}");
            Some(std::time::Duration::from_millis(u64::from(max)))
        }
        Some(ms) => Some(std::time::Duration::from_millis(ms)),
        None => {
            tracing::warn!(
                value = describe(value),
                "lock.{key} is a whole number of milliseconds, 0 or more; keeping the default, {default}"
            );
            None
        }
    }
}

fn scene(options: &Doc, has_command: bool) -> crate::lock_scene::Settings {
    use crate::lock_scene::{
        ENTER_MAX_MS, ENTER_MS, EXIT_MAX_MS, EXIT_MS, KEYPAD_MARGIN, PromptOn, SHIPPED, Settings,
    };
    let mut settings = Settings {
        native: !has_command,
        ..Settings::default()
    };
    let text = |value: &Doc| -> Option<String> { value.text().map(str::to_owned) };
    match options.get("native") {
        Doc::Bool(on) => settings.native = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "lock.native is true or false; keeping the default, true unless lock.command is set"
        ),
    }
    match options.get("scene") {
        Doc::Nil => {}
        value => match text(value).filter(|scene| !scene.is_empty()) {
            Some(scene) => settings.scene = scene,
            None => tracing::warn!(
                value = describe(value),
                "lock.scene is a path or a name, like \"{SHIPPED}\"; keeping the default"
            ),
        },
    }
    match options.get("prompt_on") {
        Doc::Nil => {}
        value => match text(value).and_then(|name| PromptOn::named(&name)) {
            Some(prompt_on) => settings.prompt_on = prompt_on,
            None => tracing::warn!(
                value = describe(value),
                "lock.prompt_on is \"primary\", \"pointer\" or a monitor's name; keeping the default, \"pointer\""
            ),
        },
    }
    if let Some(enter) = bounded_ms(
        options,
        "enter_ms",
        ENTER_MS,
        ENTER_MAX_MS,
        "the desktop is on screen until the entry ends",
    ) {
        settings.enter = enter;
    }
    if let Some(exit) = bounded_ms(
        options,
        "exit_ms",
        EXIT_MS,
        EXIT_MAX_MS,
        "input is the desktop's from the unlock on",
    ) {
        settings.exit = exit;
    }
    match options.get("keypad_margin") {
        Doc::Nil => {}
        value => {
            let whole = match &value {
                Doc::Int(number) => u32::try_from(*number).ok(),
                Doc::Num(number) if number.fract() == 0.0 && number.is_finite() => {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a whole number, bounded by the conversion below"
                    )]
                    let whole = *number as i64;
                    u32::try_from(whole).ok()
                }
                _ => None,
            };
            match whole {
                Some(margin) => settings.keypad_margin = margin,
                None => tracing::warn!(
                    value = describe(value),
                    "lock.keypad_margin is a whole number of pixels, 0 or more; keeping the default, {KEYPAD_MARGIN}"
                ),
            }
        }
    }
    settings
}
