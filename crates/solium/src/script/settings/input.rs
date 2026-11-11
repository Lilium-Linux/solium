//! `input`: libinput's settings by device type and by device, and which
//! applications may hold the keyboard shortcuts.

use smithay::reexports::input::{AccelProfile, ScrollMethod, TapButtonMap};

use super::read;
use crate::input::devices::{DeviceKind, DeviceMatch, InputConfig, Settings};
use crate::script::doc::{Doc, describe};
use crate::script::normalise_combo;
use crate::wm::convert::combo_keysyms;

/// Read the section: libinput's settings, and the shortcuts policy. A
/// shortcuts policy that cannot be read refuses the whole section, so no
/// half of it is applied.
pub(crate) fn parse(
    options: &Doc,
) -> Result<(InputConfig, crate::shortcuts_inhibit::Settings), String> {
    let mut config = InputConfig::default();
    let mut shortcuts_policy = crate::shortcuts_inhibit::Settings::default();
    if let Some(table) = read::table(options, "shortcuts_inhibit")? {
        shortcuts_policy = shortcuts(table)?;
    }
    for (kind, key) in DeviceKind::CONFIGURABLE {
        if let Some(table) = read::table(options, key)? {
            config.set_default(kind, device_settings(table, key));
        }
    }
    if let Some(rows) = read::table(options, "devices")? {
        for row in rows.list() {
            if !matches!(row, Doc::Table(_)) {
                return Err(read::not_a(row, "table"));
            }
            let name_contains = read::text(row, "name")?;
            let vendor = usb_id(row, "vendor");
            let product = usb_id(row, "product");
            let settings = device_settings(row, "devices");
            config.push_override(
                DeviceMatch {
                    name_contains,
                    vendor,
                    product,
                },
                settings,
            );
        }
    }
    Ok((config, shortcuts_policy))
}

/// Validate protocol policy before emitting any commands from `sol.input`.
pub(crate) fn shortcuts(table: &Doc) -> Result<crate::shortcuts_inhibit::Settings, String> {
    use crate::shortcuts_inhibit::{Allow, Settings};
    let refused = |why: &str| format!("input.shortcuts_inhibit: {why}");
    let allow = match table.get("allow") {
        Doc::Nil | Doc::Bool(true) => Allow::Always,
        Doc::Bool(false) => Allow::Never,
        Doc::Text(value) if value == "fullscreen" => Allow::Fullscreen,
        _ => return Err(refused("allow must be true, false or 'fullscreen'")),
    };
    let mut settings = Settings {
        allow,
        ..Settings::default()
    };
    if let Some(escape) = read::text(table, "escape")? {
        let normal = normalise_combo(&escape);
        // Validate the original components: normalization intentionally keeps
        // only the last non-modifier, which would turn `typo+q` into `q`.
        let modifiers = escape.rsplit_once('+').map(|(modifiers, _)| modifiers);
        if combo_keysyms(&escape).is_none()
            || modifiers.is_some_and(|modifiers| {
                modifiers.split('+').any(|name| {
                    !matches!(
                        name.trim().to_ascii_lowercase().as_str(),
                        "ctrl"
                            | "control"
                            | "alt"
                            | "meta"
                            | "shift"
                            | "super"
                            | "logo"
                            | "win"
                            | "cmd"
                    )
                })
            })
        {
            return Err(refused("escape must name a key combination"));
        }
        settings.escape = normal;
    }
    if let Some(apps) = read::table(table, "apps")?
        && let Some(deny) = read::texts(apps, "deny")?
    {
        for pattern in deny {
            if pattern.is_empty() || pattern.strip_suffix('*').unwrap_or(&pattern).contains('*') {
                return Err(refused(
                    "apps.deny entries must be exact names, '*' or trailing-star prefixes",
                ));
            }
            settings
                .deny
                .push(crate::effect::rules::Glob::parse(&pattern));
        }
    }
    Ok(settings)
}

/// A `devices` entry's `vendor` or `product`: a whole number from 0 to
/// `u32::MAX`, as `libinput list-devices` or `lsusb` print it. Anything else
/// is named in the log and the match criterion is left unset -- which, since
/// [`crate::input::devices::DeviceMatch::matches`] requires at least one
/// criterion, can turn an override that meant to match by vendor *and*
/// product into one that matches by product alone. Worth warning about for
/// exactly that reason.
fn usb_id(row: &Doc, key: &str) -> Option<u32> {
    match row.get(key) {
        Doc::Nil => None,
        Doc::Int(value) => match u32::try_from(*value) {
            Ok(id) => Some(id),
            Err(_) => {
                tracing::warn!(
                    key,
                    value,
                    "devices: not a usb id (0 to 0xffffffff); unmatched"
                );
                None
            }
        },
        other => {
            tracing::warn!(
                key,
                value = describe(other),
                "devices: vendor and product are numbers; unmatched"
            );
            None
        }
    }
}

/// One table of libinput options -- a device type's defaults in `config.lua`'s
/// `input` section, or one `devices` entry -- read field by field. A key left
/// out keeps that field `None` (`crate::input::devices::Settings`'s own
/// meaning: "config.lua did not say, leave it alone"); a key present but the
/// wrong shape -- `tap = "yes"`, `accel_profile = "fastest"` -- is named in
/// the log with `context` (the type's key, or `"devices"`) and left `None`
/// too, the same policy `idle` and `cursor` already use. One field being
/// wrong does not fail the rest of the table.
/// `sol_input_warns_on_an_unrecognised_option_value_and_keeps_it_unset`.
fn device_settings(table: &Doc, context: &str) -> Settings {
    let output = match table.get("output") {
        Doc::Nil => None,
        Doc::Text(text) => Some(text.clone()),
        other => {
            tracing::warn!(
                context,
                value = describe(other),
                "input.output: a connector name string; leaving it unset"
            );
            None
        }
    };
    let bool_field = |key: &str| -> Option<bool> {
        match table.get(key) {
            Doc::Nil => None,
            Doc::Bool(value) => Some(*value),
            other => {
                tracing::warn!(
                    context,
                    key,
                    value = describe(other),
                    "input: true or false; leaving it unset"
                );
                None
            }
        }
    };

    let tap_button_map = match table.get("tap_button_map") {
        Doc::Nil => None,
        Doc::Text(text) => match text.as_str() {
            "left_right_middle" => Some(TapButtonMap::LeftRightMiddle),
            "left_middle_right" => Some(TapButtonMap::LeftMiddleRight),
            other => {
                tracing::warn!(
                    context,
                    value = other,
                    "input.tap_button_map: \"left_right_middle\" or \"left_middle_right\"; \
                     leaving it unset"
                );
                None
            }
        },
        other => {
            tracing::warn!(
                context,
                value = describe(other),
                "input.tap_button_map: a string; leaving it unset"
            );
            None
        }
    };

    let scroll_method = match table.get("scroll_method") {
        Doc::Nil => None,
        Doc::Text(text) => match text.as_str() {
            "two_finger" => Some(ScrollMethod::TwoFinger),
            "edge" => Some(ScrollMethod::Edge),
            "button" => Some(ScrollMethod::OnButtonDown),
            "no_scroll" => Some(ScrollMethod::NoScroll),
            other => {
                tracing::warn!(
                    context,
                    value = other,
                    "input.scroll_method: \"two_finger\", \"edge\", \"button\" or \"no_scroll\"; \
                     leaving it unset"
                );
                None
            }
        },
        other => {
            tracing::warn!(
                context,
                value = describe(other),
                "input.scroll_method: a string; leaving it unset"
            );
            None
        }
    };

    let accel_profile = match table.get("accel_profile") {
        Doc::Nil => None,
        Doc::Text(text) => match text.as_str() {
            "flat" => Some(AccelProfile::Flat),
            "adaptive" => Some(AccelProfile::Adaptive),
            other => {
                tracing::warn!(
                    context,
                    value = other,
                    "input.accel_profile: \"flat\" or \"adaptive\"; leaving it unset"
                );
                None
            }
        },
        other => {
            tracing::warn!(
                context,
                value = describe(other),
                "input.accel_profile: a string; leaving it unset"
            );
            None
        }
    };

    let accel_speed = match table.get("accel_speed") {
        Doc::Nil => None,
        other => match other.number().filter(|speed| (-1.0..=1.0).contains(speed)) {
            Some(speed) => Some(speed),
            None => {
                tracing::warn!(
                    context,
                    value = describe(other),
                    "input.accel_speed: a number from -1 to 1; leaving it unset"
                );
                None
            }
        },
    };

    // A whole number of milliseconds from 1 to 1000 (#284): shorter follows a
    // change of direction sooner, longer smooths a noisy screen.
    let velocity_window_ms = match table.get("velocity_window_ms") {
        Doc::Nil => None,
        other => match other
            .number()
            .filter(|ms| ms.fract() == 0.0 && (1.0..=1000.0).contains(ms))
        {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked to be a whole number from 1 to 1000"
            )]
            Some(ms) => Some(ms as u32),
            None => {
                tracing::warn!(
                    context,
                    value = describe(other),
                    "input.velocity_window_ms: a whole number of milliseconds from 1 to 1000; leaving it unset"
                );
                None
            }
        },
    };

    Settings {
        output,
        velocity_window_ms,
        tap: bool_field("tap"),
        tap_button_map,
        drag: bool_field("drag"),
        drag_lock: bool_field("drag_lock"),
        natural_scroll: bool_field("natural_scroll"),
        scroll_method,
        accel_profile,
        accel_speed,
        disable_while_typing: bool_field("disable_while_typing"),
        left_handed: bool_field("left_handed"),
        middle_emulation: bool_field("middle_emulation"),
    }
}
