//! Input profiles — the seam that keeps touch from becoming a retrofit.
//!
//! One binary serves desktop, laptop, tablet and phone. What changes between
//! them is not the code path but a handful of decisions, and they are collected
//! here so that adding a form factor is a table entry rather than a branch
//! sprinkled through the event handlers.
//!
//! The form factor is read once, from `SOLIUM_FORM_FACTOR`. It becomes a real
//! setting with E8; hard-coding a default until then is fine, silently
//! scattering the decisions it implies is not.

/// What the machine is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FormFactor {
    #[default]
    Desktop,
    Laptop,
    Tablet,
    Phone,
}

impl FormFactor {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "desktop" => Some(Self::Desktop),
            "laptop" => Some(Self::Laptop),
            "tablet" => Some(Self::Tablet),
            "phone" => Some(Self::Phone),
            _ => None,
        }
    }
}

/// The modifier that turns a drag anywhere on a window into a window move.
///
/// Needed because a window without decorations has no titlebar to grab, and
/// wanted even once it has one — reaching for the titlebar to move a window is
/// a habit inherited from window managers that had no alternative.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DragModifier {
    /// Super. What tiling compositors use, and what does not collide with
    /// application shortcuts.
    #[default]
    Logo,
    /// For anyone whose Super is already spoken for.
    Alt,
}

impl DragModifier {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "logo" | "super" | "meta" => Some(Self::Logo),
            "alt" => Some(Self::Alt),
            _ => None,
        }
    }
}

/// Per-form-factor input behaviour.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Profile {
    #[expect(
        dead_code,
        reason = "E7 selects default modes per form factor; the field is the seam"
    )]
    pub(crate) form_factor: FormFactor,

    /// Pressing a pointer button on a window focuses it.
    pub(crate) click_to_focus: bool,

    /// Touching a window focuses it.
    ///
    /// Separate from `click_to_focus` on purpose: on a touch-first device the
    /// tap that dismisses a mode should not also focus whatever happened to be
    /// underneath it.
    pub(crate) touch_to_focus: bool,

    /// Moving the pointer over a window focuses it.
    ///
    /// The default on a desktop, and what every tiling compositor people
    /// arrive from does. Without it, focus has to be clicked for — which in a
    /// layout where windows are never on top of each other is a click that
    /// achieves nothing except focus.
    pub(crate) focus_follows_mouse: bool,

    /// Held to drag a window from anywhere in it.
    pub(crate) drag_modifier: DragModifier,

    /// Scroll direction follows the content rather than the surface.
    pub(crate) natural_scroll: bool,
}

impl Profile {
    /// The profile for a form factor.
    pub(crate) fn for_form_factor(form_factor: FormFactor) -> Self {
        match form_factor {
            FormFactor::Desktop => Self {
                form_factor,
                click_to_focus: true,
                touch_to_focus: true,
                focus_follows_mouse: true,
                drag_modifier: DragModifier::Logo,
                natural_scroll: false,
            },
            FormFactor::Laptop => Self {
                form_factor,
                click_to_focus: true,
                touch_to_focus: true,
                focus_follows_mouse: true,
                drag_modifier: DragModifier::Logo,
                // Trackpads are gesture surfaces, and every other trackpad on
                // this planet scrolls the content.
                natural_scroll: true,
            },
            FormFactor::Tablet | FormFactor::Phone => Self {
                form_factor,
                click_to_focus: true,
                // A tap in a mode belongs to the mode, not to what is below it.
                touch_to_focus: false,
                // There is no pointer to follow.
                focus_follows_mouse: false,
                drag_modifier: DragModifier::Logo,
                natural_scroll: true,
            },
        }
    }

    /// Read the form factor from the environment, defaulting to desktop.
    pub(crate) fn from_env() -> Self {
        let requested = std::env::var("SOLIUM_FORM_FACTOR").ok();
        let form_factor = match requested.as_deref().map(FormFactor::parse) {
            Some(Some(form_factor)) => form_factor,
            Some(None) => {
                tracing::warn!(
                    value = requested.as_deref().unwrap_or_default(),
                    "unknown SOLIUM_FORM_FACTOR, using desktop"
                );
                FormFactor::default()
            }
            None => FormFactor::default(),
        };
        let mut profile = Self::for_form_factor(form_factor);

        if let Ok(requested) = std::env::var("SOLIUM_DRAG_MODIFIER") {
            match DragModifier::parse(&requested) {
                Some(modifier) => profile.drag_modifier = modifier,
                None => tracing::warn!(
                    value = requested,
                    "unknown SOLIUM_DRAG_MODIFIER, keeping the profile default"
                ),
            }
        }

        tracing::info!(
            ?form_factor,
            drag_modifier = ?profile.drag_modifier,
            "input profile selected"
        );
        profile
    }

    /// Whether the currently held modifiers mean "drag the window".
    pub(crate) fn drag_held(&self, modifiers: &smithay::input::keyboard::ModifiersState) -> bool {
        match self.drag_modifier {
            DragModifier::Logo => modifiers.logo,
            DragModifier::Alt => modifiers.alt,
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::for_form_factor(FormFactor::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_touch_first_profile_does_not_focus_on_tap() {
        // The distinction this seam exists for: on a phone, the tap that leaves
        // a mode must not focus what was underneath.
        assert!(!Profile::for_form_factor(FormFactor::Phone).touch_to_focus);
        assert!(Profile::for_form_factor(FormFactor::Desktop).touch_to_focus);
    }

    #[test]
    fn drag_modifiers_accept_the_names_people_use() {
        assert_eq!(DragModifier::parse("super"), Some(DragModifier::Logo));
        assert_eq!(DragModifier::parse("Meta"), Some(DragModifier::Logo));
        assert_eq!(DragModifier::parse("ALT"), Some(DragModifier::Alt));
        assert_eq!(DragModifier::parse("ctrl"), None);
    }

    #[test]
    fn form_factors_parse_case_insensitively() {
        assert_eq!(FormFactor::parse("Tablet"), Some(FormFactor::Tablet));
        assert_eq!(FormFactor::parse(" phone "), Some(FormFactor::Phone));
        assert_eq!(FormFactor::parse("watch"), None);
    }
}
