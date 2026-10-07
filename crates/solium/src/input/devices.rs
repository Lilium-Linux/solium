//! libinput device settings as configuration (#157).
//!
//! Before this, Solium set no libinput device options at all: `tty.rs` only
//! counted `DeviceAdded` and logged a name, so a laptop touchpad had no
//! tap-to-click, no natural scroll, no acceleration profile -- whatever
//! libinput happened to default to on that kernel was what you got, and
//! nothing in `config.lua` could change it. The one exception,
//! `SOLIUM_FORM_FACTOR`'s natural scroll (`profile.rs`), flipped every
//! device's scroll direction in software, uniformly, whether or not it had a
//! touchpad's fingers on it.
//!
//! The split here is the one the rest of the configuration already uses:
//! **this module is the generic mechanism, in Rust** -- classify a device,
//! merge a type default with the overrides that name it, and hand the result
//! to libinput's own `config_*` setters, behind [`ApplySettings`] so a test
//! can stand in for a touchpad nobody is touching. **What the defaults and
//! the overrides actually say lives in Lua** (`sol.input`, `config.lua`'s
//! `input` section) -- this module never reads a config file and never
//! decides that a touchpad should tap.
//!
//! ## Folding in `SOLIUM_FORM_FACTOR`'s natural scroll
//!
//! Natural scroll used to be guessed from the *machine* (a laptop has a
//! touchpad, probably). Now it is decided from the *device*: `config.lua`
//! ships `natural_scroll = true` for the touchpad type, unconditionally,
//! which is strictly the question `SOLIUM_FORM_FACTOR` was standing in for --
//! a desktop with a USB touchpad plugged in now gets natural scroll too,
//! which the old guess could never give it. Once libinput has set a device's
//! natural scroll, the raw axis values it reports are already flipped, so
//! `input/mod.rs`'s software flip must not run a second time on top of it, or
//! the two cancel out and the touchpad scrolls the *traditional* way despite
//! both settings agreeing it should not. See `pointer_axis` in `input/mod.rs`
//! for the other half of that: it asks [`Registry::handled`] before falling
//! back to `profile.natural_scroll`, so a device this module never touches --
//! because it is not a touchpad, or because libinput refused the setting, or
//! because this is the nested backend, which has no libinput devices at all
//! -- keeps exactly the behaviour it had before #157.
//!
//! ## What only real hardware can prove
//!
//! Every mapping and merge here is unit-tested against a fake device
//! (`tests::fake`), because that is what "the libinput calls go through a
//! mock" (the issue's own words) asks for. No test opens an evdev node. A
//! real touchpad is the only thing that can confirm libinput actually heard
//! the setter and tapping now clicks -- see the handover note in the PR this
//! lands in.

use std::collections::HashMap;

use smithay::backend::input::{Device as BackendDevice, DeviceCapability};
use smithay::reexports::input::{AccelProfile, DeviceConfigResult, ScrollMethod, TapButtonMap};

/// What kind of device libinput handed us, by capability.
///
/// Libiput itself does not name a "touchpad" or a "mouse" -- both are just
/// `Pointer`. The distinguishing capability is `Gesture`: libinput only
/// reports it for a device whose driver recognises multi-finger gestures,
/// which in practice is touchpads and nothing else. Checked before plain
/// `Pointer` so a touchpad is never misclassified as a mouse.
/// `a_pointer_with_gesture_capability_is_a_touchpad_and_without_it_a_mouse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DeviceKind {
    Touchpad,
    Mouse,
    Keyboard,
    Touchscreen,
    TabletTool,
    TabletPad,
    Switch,
    /// A capability combination none of the above matches: a device
    /// `config.lua` has no type-shaped default for, though a `devices`
    /// override naming it by vendor/product still applies
    /// (`an_override_reaches_a_device_of_no_configured_type`).
    Other,
}

impl DeviceKind {
    /// Classify by capability, most specific first: a tablet's pad and a
    /// touchscreen both happen to report capabilities a careless order would
    /// read as a pointer or a switch.
    pub(crate) fn classify(device: &impl BackendDevice) -> Self {
        if device.has_capability(DeviceCapability::TabletTool) {
            Self::TabletTool
        } else if device.has_capability(DeviceCapability::TabletPad) {
            Self::TabletPad
        } else if device.has_capability(DeviceCapability::Touch) {
            Self::Touchscreen
        } else if device.has_capability(DeviceCapability::Pointer) {
            if device.has_capability(DeviceCapability::Gesture) {
                Self::Touchpad
            } else {
                Self::Mouse
            }
        } else if device.has_capability(DeviceCapability::Keyboard) {
            Self::Keyboard
        } else if device.has_capability(DeviceCapability::Switch) {
            Self::Switch
        } else {
            Self::Other
        }
    }

    /// Every type `config.lua`'s `input` section has a default for, paired
    /// with the key it is written under. `Other` is deliberately excluded --
    /// see its doc comment.
    pub(crate) const CONFIGURABLE: [(Self, &'static str); 7] = [
        (Self::Touchpad, "touchpad"),
        (Self::Mouse, "mouse"),
        (Self::Keyboard, "keyboard"),
        (Self::Touchscreen, "touchscreen"),
        (Self::TabletTool, "tablet_tool"),
        (Self::TabletPad, "tablet_pad"),
        (Self::Switch, "switch"),
    ];

    /// The word `--check` and the log line name this kind by.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Touchpad => "touchpad",
            Self::Mouse => "mouse",
            Self::Keyboard => "keyboard",
            Self::Touchscreen => "touchscreen",
            Self::TabletTool => "tablet_tool",
            Self::TabletPad => "tablet_pad",
            Self::Switch => "switch",
            Self::Other => "other",
        }
    }
}

/// A device as `config.lua`'s matching rules see it: enough to test a name
/// substring or a vendor/product pair against, and nothing that needs a live
/// libinput handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceInfo {
    /// libinput's `sysname` -- stable for as long as the device is plugged
    /// in, and what both the reload re-apply registry and `pointer_axis`'s
    /// natural-scroll lookup key on.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: DeviceKind,
    pub(crate) vendor: Option<u32>,
    pub(crate) product: Option<u32>,
}

impl DeviceInfo {
    pub(crate) fn of(device: &impl BackendDevice) -> Self {
        // `Device::usb_id` hands back `(product, vendor)` -- that order, not
        // the one every USB listing prints them in -- so this is written out
        // rather than trusted to a tuple pattern read at a glance.
        let (product, vendor) = device.usb_id().unzip();
        Self {
            id: device.id(),
            name: device.name(),
            kind: DeviceKind::classify(device),
            vendor,
            product,
        }
    }
}

/// One device's settings, each left `None` meaning "config.lua did not say,
/// leave libinput's own default (or whatever a previous reload already set)
/// alone" -- never "turn this off". There is no libinput call that means
/// "forget what I set", so a field a reload stops mentioning is left as it
/// was rather than reset to a hardware default nothing here could even name.
/// `a_reload_that_stops_mentioning_a_field_leaves_the_device_as_it_was`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Settings {
    pub(crate) tap: Option<bool>,
    pub(crate) tap_button_map: Option<TapButtonMap>,
    pub(crate) drag: Option<bool>,
    pub(crate) drag_lock: Option<bool>,
    pub(crate) natural_scroll: Option<bool>,
    pub(crate) scroll_method: Option<ScrollMethod>,
    pub(crate) accel_profile: Option<AccelProfile>,
    pub(crate) accel_speed: Option<f64>,
    pub(crate) disable_while_typing: Option<bool>,
    pub(crate) left_handed: Option<bool>,
    pub(crate) middle_emulation: Option<bool>,
}

impl Settings {
    /// Every field `over` actually sets replaces this one's; a field `over`
    /// leaves `None` keeps whatever this already had. The merge a type
    /// default and a chain of matching overrides are folded with --
    /// `resolve` calls this once per matching override, in `config.lua`'s
    /// `devices` order, so the last override to mention a field wins and one
    /// that says nothing about it never erases an earlier match's answer.
    /// `two_overrides_matching_the_same_device_merge_field_by_field_in_order`.
    fn overlay(&mut self, over: &Self) {
        if over.tap.is_some() {
            self.tap = over.tap;
        }
        if over.tap_button_map.is_some() {
            self.tap_button_map = over.tap_button_map;
        }
        if over.drag.is_some() {
            self.drag = over.drag;
        }
        if over.drag_lock.is_some() {
            self.drag_lock = over.drag_lock;
        }
        if over.natural_scroll.is_some() {
            self.natural_scroll = over.natural_scroll;
        }
        if over.scroll_method.is_some() {
            self.scroll_method = over.scroll_method;
        }
        if over.accel_profile.is_some() {
            self.accel_profile = over.accel_profile;
        }
        if over.accel_speed.is_some() {
            self.accel_speed = over.accel_speed;
        }
        if over.disable_while_typing.is_some() {
            self.disable_while_typing = over.disable_while_typing;
        }
        if over.left_handed.is_some() {
            self.left_handed = over.left_handed;
        }
        if over.middle_emulation.is_some() {
            self.middle_emulation = over.middle_emulation;
        }
    }
}

/// One `config.lua` `devices` entry's match half: a name substring, a
/// vendor/product pair, or both. At least one of the three must be given --
/// an empty `{}` matches nothing rather than everything, which is what keeps
/// a typo'd match from silently reaching every device on the machine.
/// `an_override_with_no_match_criteria_at_all_matches_nothing`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeviceMatch {
    /// Matched case-insensitively, as a substring: `"logitech"` reaches
    /// "Logitech MX Master 3S" without the whole name written out.
    pub(crate) name_contains: Option<String>,
    pub(crate) vendor: Option<u32>,
    pub(crate) product: Option<u32>,
}

impl DeviceMatch {
    pub(crate) fn matches(&self, info: &DeviceInfo) -> bool {
        let named = self.name_contains.is_some() || self.vendor.is_some() || self.product.is_some();
        named
            && self.name_contains.as_deref().is_none_or(|needle| {
                info.name
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
            && self.vendor.is_none_or(|vendor| info.vendor == Some(vendor))
            && self
                .product
                .is_none_or(|product| info.product == Some(product))
    }
}

/// What `sol.input{...}` said: a default per device type, and overrides
/// matched by name or by vendor/product, in the order `config.lua`'s
/// `devices` list gave them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct InputConfig {
    defaults: HashMap<DeviceKind, Settings>,
    overrides: Vec<(DeviceMatch, Settings)>,
}

impl InputConfig {
    pub(crate) fn set_default(&mut self, kind: DeviceKind, settings: Settings) {
        self.defaults.insert(kind, settings);
    }

    pub(crate) fn push_override(&mut self, matcher: DeviceMatch, settings: Settings) {
        self.overrides.push((matcher, settings));
    }

    /// The settings one device gets: its type's default, with every
    /// override that names it folded over in turn.
    /// `a_device_with_no_matching_override_gets_only_its_type_default`,
    /// `a_name_matched_override_changes_only_the_fields_it_names`,
    /// `a_vendor_and_product_matched_override_applies`.
    pub(crate) fn resolve(&self, info: &DeviceInfo) -> Settings {
        let mut settings = self.defaults.get(&info.kind).copied().unwrap_or_default();
        for (matcher, over) in &self.overrides {
            if matcher.matches(info) {
                settings.overlay(over);
            }
        }
        settings
    }
}

/// Whether a setting actually reached the device, or the device (or this
/// libinput build) does not support it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    Set,
    Unsupported,
}

fn outcome(result: DeviceConfigResult) -> Applied {
    match result {
        Ok(()) => Applied::Set,
        Err(_) => Applied::Unsupported,
    }
}

/// What [`apply`] did, one entry per field `Settings` actually named --
/// nothing for a field left `None`, since "not configured" is not "tried and
/// failed". `tracing` turns this into the one log line per device the issue
/// asks for; `sol.on("input_device", ...)` turns it into a table.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Report(Vec<(&'static str, Applied)>);

impl Report {
    pub(crate) fn applied(&self) -> Vec<&'static str> {
        self.0
            .iter()
            .filter(|(_, applied)| *applied == Applied::Set)
            .map(|(field, _)| *field)
            .collect()
    }

    pub(crate) fn unsupported(&self) -> Vec<&'static str> {
        self.0
            .iter()
            .filter(|(_, applied)| *applied == Applied::Unsupported)
            .map(|(field, _)| *field)
            .collect()
    }
}

/// What [`apply`] needs from a device: one setter per option the issue
/// lists, each returning whether it took. Implemented for the real libinput
/// `Device` below, and for `tests::fake::FakeDevice` -- which is the whole
/// reason this is a trait and not a plain function over `input::Device`:
/// "the libinput calls go through a mock" is the issue's own test
/// requirement, and a free function reaching into a real `Device` could
/// never be called without one.
pub(crate) trait ApplySettings {
    fn set_tap_enabled(&mut self, enabled: bool) -> Applied;
    fn set_tap_button_map(&mut self, map: TapButtonMap) -> Applied;
    fn set_tap_drag_enabled(&mut self, enabled: bool) -> Applied;
    fn set_tap_drag_lock_enabled(&mut self, enabled: bool) -> Applied;
    fn set_natural_scroll_enabled(&mut self, enabled: bool) -> Applied;
    fn set_scroll_method(&mut self, method: ScrollMethod) -> Applied;
    fn set_accel_profile(&mut self, profile: AccelProfile) -> Applied;
    fn set_accel_speed(&mut self, speed: f64) -> Applied;
    fn set_disable_while_typing(&mut self, enabled: bool) -> Applied;
    fn set_left_handed(&mut self, enabled: bool) -> Applied;
    fn set_middle_emulation(&mut self, enabled: bool) -> Applied;
}

/// Hand a resolved `Settings` to a device, field by field, skipping every
/// one left `None`. Called from `DeviceAdded` and again, with whatever
/// `resolve` now answers, on every reload
/// (`a_reload_that_changes_an_override_reapplies_it_to_the_same_device`).
pub(crate) fn apply(device: &mut impl ApplySettings, settings: &Settings) -> Report {
    let mut report = Vec::new();
    if let Some(enabled) = settings.tap {
        report.push(("tap", device.set_tap_enabled(enabled)));
    }
    if let Some(map) = settings.tap_button_map {
        report.push(("tap_button_map", device.set_tap_button_map(map)));
    }
    if let Some(enabled) = settings.drag {
        report.push(("drag", device.set_tap_drag_enabled(enabled)));
    }
    if let Some(enabled) = settings.drag_lock {
        report.push(("drag_lock", device.set_tap_drag_lock_enabled(enabled)));
    }
    if let Some(enabled) = settings.natural_scroll {
        report.push(("natural_scroll", device.set_natural_scroll_enabled(enabled)));
    }
    if let Some(method) = settings.scroll_method {
        report.push(("scroll_method", device.set_scroll_method(method)));
    }
    if let Some(profile) = settings.accel_profile {
        report.push(("accel_profile", device.set_accel_profile(profile)));
    }
    if let Some(speed) = settings.accel_speed {
        report.push(("accel_speed", device.set_accel_speed(speed)));
    }
    if let Some(enabled) = settings.disable_while_typing {
        report.push((
            "disable_while_typing",
            device.set_disable_while_typing(enabled),
        ));
    }
    if let Some(enabled) = settings.left_handed {
        report.push(("left_handed", device.set_left_handed(enabled)));
    }
    if let Some(enabled) = settings.middle_emulation {
        report.push(("middle_emulation", device.set_middle_emulation(enabled)));
    }
    Report(report)
}

impl ApplySettings for smithay::reexports::input::Device {
    fn set_tap_enabled(&mut self, enabled: bool) -> Applied {
        outcome(self.config_tap_set_enabled(enabled))
    }

    fn set_tap_button_map(&mut self, map: TapButtonMap) -> Applied {
        outcome(self.config_tap_set_button_map(map))
    }

    fn set_tap_drag_enabled(&mut self, enabled: bool) -> Applied {
        outcome(self.config_tap_set_drag_enabled(enabled))
    }

    fn set_tap_drag_lock_enabled(&mut self, enabled: bool) -> Applied {
        outcome(self.config_tap_set_drag_lock_enabled(enabled))
    }

    fn set_natural_scroll_enabled(&mut self, enabled: bool) -> Applied {
        outcome(self.config_scroll_set_natural_scroll_enabled(enabled))
    }

    fn set_scroll_method(&mut self, method: ScrollMethod) -> Applied {
        outcome(self.config_scroll_set_method(method))
    }

    fn set_accel_profile(&mut self, profile: AccelProfile) -> Applied {
        outcome(self.config_accel_set_profile(profile))
    }

    fn set_accel_speed(&mut self, speed: f64) -> Applied {
        outcome(self.config_accel_set_speed(speed))
    }

    fn set_disable_while_typing(&mut self, enabled: bool) -> Applied {
        outcome(self.config_dwt_set_enabled(enabled))
    }

    fn set_left_handed(&mut self, enabled: bool) -> Applied {
        outcome(self.config_left_handed_set(enabled))
    }

    fn set_middle_emulation(&mut self, enabled: bool) -> Applied {
        outcome(self.config_middle_emulation_set_enabled(enabled))
    }
}

/// Every device met since start, and what `config.lua` says about devices
/// not yet seen. Backend-agnostic on purpose, like the rest of `Solium`: the
/// nested backend never calls [`Registry::device_seen`], so `devices()` and
/// `handled` are simply always empty there, which is the correct answer
/// (there is no libinput device to have configured).
#[derive(Debug, Default)]
pub(crate) struct Registry {
    config: InputConfig,
    known: Vec<(DeviceInfo, Report)>,
}

impl Registry {
    pub(crate) fn configure(&mut self, config: InputConfig) {
        self.config = config;
    }

    pub(crate) fn config(&self) -> &InputConfig {
        &self.config
    }

    pub(crate) fn device_seen(&mut self, info: DeviceInfo, report: Report) {
        self.known.retain(|(existing, _)| existing.id != info.id);
        self.known.push((info, report));
    }

    pub(crate) fn device_gone(&mut self, id: &str) {
        self.known.retain(|(existing, _)| existing.id != id);
    }

    /// Test-only: nothing reads the list back yet (`sol.input_devices()` is
    /// the deliberate cut the PR this lands in names), but `device_gone`
    /// removing the right entry needs some way to look.
    #[cfg(test)]
    fn devices(&self) -> &[(DeviceInfo, Report)] {
        &self.known
    }

    /// Whether `id`'s natural scroll is this mechanism's to answer for --
    /// `config.lua` named it, and the device accepted the setting -- which is
    /// exactly when `input/mod.rs`'s software flip must stand down (see the
    /// module doc's "Folding in `SOLIUM_FORM_FACTOR`"). Not *what* was set:
    /// libinput already inverted the raw deltas we go on to read, so the
    /// right answer here is always "leave it alone", never a second flip.
    /// `pointer_axis_does_not_flip_a_device_whose_natural_scroll_libinput_already_set`.
    pub(crate) fn handled(&self, id: &str) -> bool {
        self.known
            .iter()
            .any(|(info, report)| info.id == id && report.applied().contains(&"natural_scroll"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device no test ever has to open an evdev node for: capabilities set
    /// by hand, a name and a usb id given up front. Stands in for
    /// `smithay::backend::input::Device` wherever this module reads one.
    #[derive(Clone, Debug, Default)]
    struct FakeBackendDevice {
        id: String,
        name: String,
        capabilities: Vec<DeviceCapability>,
        usb_id: Option<(u32, u32)>,
    }

    impl PartialEq for FakeBackendDevice {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }
    impl Eq for FakeBackendDevice {}
    impl std::hash::Hash for FakeBackendDevice {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            self.id.hash(state);
        }
    }

    impl BackendDevice for FakeBackendDevice {
        fn id(&self) -> String {
            self.id.clone()
        }
        fn name(&self) -> String {
            self.name.clone()
        }
        fn has_capability(&self, capability: DeviceCapability) -> bool {
            self.capabilities.contains(&capability)
        }
        fn usb_id(&self) -> Option<(u32, u32)> {
            self.usb_id
        }
        fn syspath(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    fn touchpad(name: &str) -> FakeBackendDevice {
        FakeBackendDevice {
            id: name.to_ascii_lowercase(),
            name: name.to_owned(),
            capabilities: vec![DeviceCapability::Pointer, DeviceCapability::Gesture],
            usb_id: None,
        }
    }

    fn mouse(name: &str, usb_id: Option<(u32, u32)>) -> FakeBackendDevice {
        FakeBackendDevice {
            id: name.to_ascii_lowercase(),
            name: name.to_owned(),
            capabilities: vec![DeviceCapability::Pointer],
            usb_id,
        }
    }

    #[test]
    fn a_pointer_with_gesture_capability_is_a_touchpad_and_without_it_a_mouse() {
        assert_eq!(
            DeviceKind::classify(&touchpad("Synaptics TouchPad")),
            DeviceKind::Touchpad
        );
        assert_eq!(
            DeviceKind::classify(&mouse("Logitech MX Master 3S", None)),
            DeviceKind::Mouse
        );
    }

    #[test]
    fn a_keyboard_and_a_touchscreen_classify_by_their_own_capability() {
        let keyboard = FakeBackendDevice {
            id: "kbd".into(),
            name: "AT Translated Set 2 keyboard".into(),
            capabilities: vec![DeviceCapability::Keyboard],
            usb_id: None,
        };
        let touchscreen = FakeBackendDevice {
            id: "touch".into(),
            name: "ELAN Touchscreen".into(),
            capabilities: vec![DeviceCapability::Touch],
            usb_id: None,
        };
        assert_eq!(DeviceKind::classify(&keyboard), DeviceKind::Keyboard);
        assert_eq!(DeviceKind::classify(&touchscreen), DeviceKind::Touchscreen);
    }

    /// `Device::usb_id` hands back `(product, vendor)`. A `DeviceInfo` built
    /// the other way round would match a `devices` override by the wrong
    /// field and nobody would notice until the override reached the wrong
    /// device.
    #[test]
    fn device_info_reads_usb_id_as_product_then_vendor() {
        let info = DeviceInfo::of(&mouse("Logitech MX Master 3S", Some((0xc52b, 0x046d))));
        assert_eq!(info.product, Some(0xc52b));
        assert_eq!(info.vendor, Some(0x046d));
    }

    fn settings_with_tap_and_scroll(tap: bool, natural_scroll: bool) -> Settings {
        Settings {
            tap: Some(tap),
            natural_scroll: Some(natural_scroll),
            ..Settings::default()
        }
    }

    #[test]
    fn a_device_with_no_matching_override_gets_only_its_type_default() {
        let mut config = InputConfig::default();
        config.set_default(
            DeviceKind::Touchpad,
            settings_with_tap_and_scroll(true, true),
        );
        let info = DeviceInfo::of(&touchpad("Synaptics TouchPad"));
        assert_eq!(
            config.resolve(&info),
            settings_with_tap_and_scroll(true, true)
        );
    }

    #[test]
    fn a_name_matched_override_changes_only_the_fields_it_names() {
        let mut config = InputConfig::default();
        config.set_default(
            DeviceKind::Touchpad,
            settings_with_tap_and_scroll(true, true),
        );
        config.push_override(
            DeviceMatch {
                name_contains: Some("synaptics".to_owned()),
                ..DeviceMatch::default()
            },
            Settings {
                natural_scroll: Some(false),
                ..Settings::default()
            },
        );
        let info = DeviceInfo::of(&touchpad("Synaptics TouchPad"));
        let resolved = config.resolve(&info);
        assert_eq!(
            resolved.tap,
            Some(true),
            "the override said nothing about tap"
        );
        assert_eq!(
            resolved.natural_scroll,
            Some(false),
            "the override's own field"
        );
    }

    #[test]
    fn a_vendor_and_product_matched_override_applies() {
        let mut config = InputConfig::default();
        config.push_override(
            DeviceMatch {
                vendor: Some(0x046d),
                product: Some(0xc52b),
                ..DeviceMatch::default()
            },
            Settings {
                accel_speed: Some(0.4),
                ..Settings::default()
            },
        );
        let matching = DeviceInfo::of(&mouse("Logitech MX Master 3S", Some((0xc52b, 0x046d))));
        let other = DeviceInfo::of(&mouse("Some Other Mouse", Some((0x1234, 0x5678))));
        assert_eq!(config.resolve(&matching).accel_speed, Some(0.4));
        assert_eq!(config.resolve(&other).accel_speed, None);
    }

    #[test]
    fn two_overrides_matching_the_same_device_merge_field_by_field_in_order() {
        let mut config = InputConfig::default();
        let matches_everything = DeviceMatch {
            name_contains: Some("mouse".to_owned()),
            ..DeviceMatch::default()
        };
        config.push_override(
            matches_everything.clone(),
            Settings {
                accel_speed: Some(0.1),
                left_handed: Some(true),
                ..Settings::default()
            },
        );
        config.push_override(
            matches_everything,
            Settings {
                accel_speed: Some(0.9),
                ..Settings::default()
            },
        );
        let resolved = config.resolve(&DeviceInfo::of(&mouse("Any Mouse", None)));
        assert_eq!(
            resolved.accel_speed,
            Some(0.9),
            "the later override's own field wins"
        );
        assert_eq!(
            resolved.left_handed,
            Some(true),
            "the later override said nothing about it, so the earlier one's answer stands"
        );
    }

    #[test]
    fn an_override_with_no_match_criteria_at_all_matches_nothing() {
        let empty = DeviceMatch::default();
        assert!(!empty.matches(&DeviceInfo::of(&mouse("Anything", None))));
    }

    /// `Other` has no key in `config.lua`'s `input` section -- `CONFIGURABLE`
    /// deliberately excludes it, so `InputConfig::defaults` can never hold an
    /// entry for it -- but a `devices` override naming a device by vendor or
    /// product still has to reach it: the whole reason overrides are matched
    /// by identity rather than by type.
    #[test]
    fn an_override_reaches_a_device_of_no_configured_type() {
        let mut config = InputConfig::default();
        config.push_override(
            DeviceMatch {
                vendor: Some(0x1234),
                product: Some(0x0001),
                ..DeviceMatch::default()
            },
            Settings {
                left_handed: Some(true),
                ..Settings::default()
            },
        );
        // No capability this module recognises: classifies as `Other`, and
        // `config.defaults` has nothing filed under that key at all.
        let unrecognised = FakeBackendDevice {
            id: "mystery".into(),
            name: "Unrecognised Device".into(),
            capabilities: Vec::new(),
            usb_id: Some((0x0001, 0x1234)),
        };
        assert_eq!(DeviceKind::classify(&unrecognised), DeviceKind::Other);
        assert_eq!(
            config.resolve(&DeviceInfo::of(&unrecognised)).left_handed,
            Some(true),
            "a vendor/product override must reach a device even when its type has no default"
        );
    }

    /// The fake `ApplySettings` the "libinput calls go through a mock" tests
    /// run against: records what was actually set, and can be told to refuse
    /// a named field the way a real device without that capability would.
    #[derive(Debug, Default)]
    pub(super) struct FakeDevice {
        pub(super) got: Settings,
        refuses: Vec<&'static str>,
    }

    impl FakeDevice {
        fn refusing(fields: &[&'static str]) -> Self {
            Self {
                got: Settings::default(),
                refuses: fields.to_vec(),
            }
        }

        fn outcome_for(&self, field: &'static str) -> Applied {
            if self.refuses.contains(&field) {
                Applied::Unsupported
            } else {
                Applied::Set
            }
        }
    }

    impl ApplySettings for FakeDevice {
        fn set_tap_enabled(&mut self, enabled: bool) -> Applied {
            let applied = self.outcome_for("tap");
            if applied == Applied::Set {
                self.got.tap = Some(enabled);
            }
            applied
        }
        fn set_tap_button_map(&mut self, map: TapButtonMap) -> Applied {
            let applied = self.outcome_for("tap_button_map");
            if applied == Applied::Set {
                self.got.tap_button_map = Some(map);
            }
            applied
        }
        fn set_tap_drag_enabled(&mut self, enabled: bool) -> Applied {
            let applied = self.outcome_for("drag");
            if applied == Applied::Set {
                self.got.drag = Some(enabled);
            }
            applied
        }
        fn set_tap_drag_lock_enabled(&mut self, enabled: bool) -> Applied {
            let applied = self.outcome_for("drag_lock");
            if applied == Applied::Set {
                self.got.drag_lock = Some(enabled);
            }
            applied
        }
        fn set_natural_scroll_enabled(&mut self, enabled: bool) -> Applied {
            let applied = self.outcome_for("natural_scroll");
            if applied == Applied::Set {
                self.got.natural_scroll = Some(enabled);
            }
            applied
        }
        fn set_scroll_method(&mut self, method: ScrollMethod) -> Applied {
            let applied = self.outcome_for("scroll_method");
            if applied == Applied::Set {
                self.got.scroll_method = Some(method);
            }
            applied
        }
        fn set_accel_profile(&mut self, profile: AccelProfile) -> Applied {
            let applied = self.outcome_for("accel_profile");
            if applied == Applied::Set {
                self.got.accel_profile = Some(profile);
            }
            applied
        }
        fn set_accel_speed(&mut self, speed: f64) -> Applied {
            let applied = self.outcome_for("accel_speed");
            if applied == Applied::Set {
                self.got.accel_speed = Some(speed);
            }
            applied
        }
        fn set_disable_while_typing(&mut self, enabled: bool) -> Applied {
            let applied = self.outcome_for("disable_while_typing");
            if applied == Applied::Set {
                self.got.disable_while_typing = Some(enabled);
            }
            applied
        }
        fn set_left_handed(&mut self, enabled: bool) -> Applied {
            let applied = self.outcome_for("left_handed");
            if applied == Applied::Set {
                self.got.left_handed = Some(enabled);
            }
            applied
        }
        fn set_middle_emulation(&mut self, enabled: bool) -> Applied {
            let applied = self.outcome_for("middle_emulation");
            if applied == Applied::Set {
                self.got.middle_emulation = Some(enabled);
            }
            applied
        }
    }

    #[test]
    fn apply_calls_only_the_setters_for_fields_the_settings_actually_name() {
        let mut device = FakeDevice::default();
        let settings = Settings {
            tap: Some(true),
            natural_scroll: Some(true),
            ..Settings::default()
        };
        let report = apply(&mut device, &settings);
        assert_eq!(device.got.tap, Some(true));
        assert_eq!(device.got.natural_scroll, Some(true));
        assert_eq!(device.got.accel_speed, None, "never named, never called");
        let mut applied = report.applied();
        applied.sort_unstable();
        assert_eq!(applied, ["natural_scroll", "tap"]);
    }

    #[test]
    fn a_field_the_device_refuses_is_reported_unsupported_and_left_unset() {
        let mut device = FakeDevice::refusing(&["tap"]);
        let report = apply(
            &mut device,
            &Settings {
                tap: Some(true),
                ..Settings::default()
            },
        );
        assert_eq!(report.unsupported(), ["tap"]);
        assert_eq!(
            device.got.tap, None,
            "a setter that refused did not quietly record the value anyway"
        );
    }

    #[test]
    fn a_reload_that_changes_an_override_reapplies_it_to_the_same_device() {
        let mut device = FakeDevice::default();
        let info = DeviceInfo::of(&touchpad("Synaptics TouchPad"));

        let mut before = InputConfig::default();
        before.set_default(
            DeviceKind::Touchpad,
            Settings {
                tap: Some(true),
                natural_scroll: Some(true),
                ..Settings::default()
            },
        );
        apply(&mut device, &before.resolve(&info));
        assert_eq!(device.got.tap, Some(true));
        assert_eq!(device.got.natural_scroll, Some(true));

        // `user.lua` edited between the two: tap turned off, natural scroll
        // left unmentioned.
        let mut after = InputConfig::default();
        after.set_default(
            DeviceKind::Touchpad,
            Settings {
                tap: Some(false),
                ..Settings::default()
            },
        );
        apply(&mut device, &after.resolve(&info));
        assert_eq!(
            device.got.tap,
            Some(false),
            "the reload's own change reached the device"
        );
        assert_eq!(
            device.got.natural_scroll,
            Some(true),
            "the reload said nothing about it, so the device keeps what the first apply set -- \
             libinput has no call that means \"forget this and go back to default\""
        );
    }

    #[test]
    fn registry_reports_a_device_as_handled_only_after_natural_scroll_was_actually_set() {
        let mut registry = Registry::default();
        let mut device = FakeDevice::default();
        let info = DeviceInfo::of(&touchpad("Synaptics TouchPad"));
        let settings = Settings {
            natural_scroll: Some(true),
            ..Settings::default()
        };
        let report = apply(&mut device, &settings);
        registry.device_seen(info.clone(), report);
        assert!(registry.handled(&info.id));

        let mouse_info = DeviceInfo::of(&mouse("Plain Mouse", None));
        registry.device_seen(mouse_info.clone(), Report::default());
        assert!(
            !registry.handled(&mouse_info.id),
            "nothing set this device's natural scroll, so the software flip must still run for it"
        );
    }

    #[test]
    fn device_gone_removes_a_device_the_registry_no_longer_has() {
        let mut registry = Registry::default();
        let info = DeviceInfo::of(&touchpad("Synaptics TouchPad"));
        registry.device_seen(info.clone(), Report::default());
        assert_eq!(registry.devices().len(), 1);
        registry.device_gone(&info.id);
        assert!(registry.devices().is_empty());
    }
}
