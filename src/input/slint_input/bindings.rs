//! `--mouse-bind`: which button does what, and what Android calls it.
//!
//! Self-contained on purpose — it turns a four-character spec into a lookup and
//! knows nothing about sessions, controllers or windows.

/// Mouse button ids as `ui/mirror.slint` reports them.
pub(super) const BUTTON_LEFT: i32 = 1;
pub(super) const BUTTON_RIGHT: i32 = 2;
pub(super) const BUTTON_MIDDLE: i32 = 3;

/// Slint numbers its buttons 1, 2, 3; Android's `MotionEvent` uses a bit per
/// button, and 2 there is the middle one rather than the right. Forwarding a
/// secondary click used to send `1` whatever had been clicked, so a right-click
/// arrived at the device as a left-click.
pub(super) fn android_button(button: i32) -> u32 {
    match button {
        BUTTON_LEFT => 1,   // BUTTON_PRIMARY
        BUTTON_RIGHT => 2,  // BUTTON_SECONDARY
        BUTTON_MIDDLE => 4, // BUTTON_TERTIARY
        _ => 0,
    }
}

/// What a secondary click does, from `--mouse-bind`.
///
/// scrcpy takes one or two four-character sequences — right, middle, 4th, 5th —
/// where the second applies while Shift is held. The default for SDK injection
/// is `bhsn:++++`, which is what this client always did in hard-coded form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SecondaryClick {
    Forward,
    Ignore,
    Back,
    Home,
    AppSwitch,
    Notifications,
}

impl SecondaryClick {
    fn parse(c: char) -> Option<Self> {
        Some(match c {
            '+' => SecondaryClick::Forward,
            '-' => SecondaryClick::Ignore,
            'b' => SecondaryClick::Back,
            'h' => SecondaryClick::Home,
            's' => SecondaryClick::AppSwitch,
            'n' => SecondaryClick::Notifications,
            _ => return None,
        })
    }
}

/// The four secondary buttons, with and without Shift.
#[derive(Debug, Clone, Copy)]
pub struct MouseBindings {
    primary: [SecondaryClick; 4],
    shifted: [SecondaryClick; 4],
}

impl Default for MouseBindings {
    fn default() -> Self {
        // scrcpy's SDK default: bhsn:++++
        Self {
            primary: [
                SecondaryClick::Back,
                SecondaryClick::Home,
                SecondaryClick::AppSwitch,
                SecondaryClick::Notifications,
            ],
            shifted: [SecondaryClick::Forward; 4],
        }
    }
}

impl MouseBindings {
    /// Parse `xxxx` or `xxxx:xxxx`; anything malformed keeps the default and
    /// says so, rather than silently mirroring with the wrong buttons.
    pub fn parse(spec: &str) -> Self {
        let mut bindings = MouseBindings::default();
        if spec.is_empty() {
            return bindings;
        }

        let (first, second) = match spec.split_once(':') {
            Some((a, b)) => (a, b),
            None => (spec, spec),
        };

        let sequence = |text: &str| -> Option<[SecondaryClick; 4]> {
            let parsed: Vec<SecondaryClick> =
                text.chars().filter_map(SecondaryClick::parse).collect();
            (parsed.len() == 4 && text.chars().count() == 4)
                .then(|| [parsed[0], parsed[1], parsed[2], parsed[3]])
        };

        match (sequence(first), sequence(second)) {
            (Some(primary), Some(shifted)) => {
                bindings.primary = primary;
                bindings.shifted = shifted;
            }
            _ => log::warn!(
                "--mouse-bind={spec} is not two 4-character sequences of +-bhsn; \
                 keeping the default"
            ),
        }
        bindings
    }

    pub(super) fn for_button(&self, button: i32, shift: bool) -> Option<SecondaryClick> {
        let index = match button {
            BUTTON_RIGHT => 0,
            BUTTON_MIDDLE => 1,
            4 => 2,
            5 => 3,
            _ => return None,
        };
        Some(if shift {
            self.shifted[index]
        } else {
            self.primary[index]
        })
    }
}

#[cfg(test)]
mod tests {
    /// `--mouse-bind`'s forward setting has to forward the button that was
    /// clicked. Slint numbers its buttons 1, 2, 3 and Android uses a bit per
    /// button, where 2 is the middle one — and a right-click used to be sent as
    /// the primary button whichever it had been, so "forward" did something
    /// other than forward.
    #[test]
    fn a_forwarded_button_is_the_button_that_was_clicked() {
        assert_eq!(super::android_button(super::BUTTON_LEFT), 1);
        assert_eq!(super::android_button(super::BUTTON_RIGHT), 2);
        assert_eq!(
            super::android_button(super::BUTTON_MIDDLE),
            4,
            "the middle button is the third bit, not the second"
        );
        assert_eq!(super::android_button(9), 0, "a button Android has no bit for");
    }

    use super::*;

    #[test]
    fn the_default_bindings_are_what_this_client_always_did() {
        let bindings = MouseBindings::default();
        assert_eq!(bindings.for_button(BUTTON_RIGHT, false), Some(SecondaryClick::Back));
        assert_eq!(bindings.for_button(BUTTON_MIDDLE, false), Some(SecondaryClick::Home));
        assert_eq!(bindings.for_button(4, false), Some(SecondaryClick::AppSwitch));
        assert_eq!(bindings.for_button(5, false), Some(SecondaryClick::Notifications));
    }

    #[test]
    fn shift_uses_the_second_sequence() {
        let bindings = MouseBindings::parse("bhsn:++++");
        assert_eq!(bindings.for_button(BUTTON_RIGHT, false), Some(SecondaryClick::Back));
        assert_eq!(bindings.for_button(BUTTON_RIGHT, true), Some(SecondaryClick::Forward));
    }

    #[test]
    fn one_sequence_applies_to_both() {
        let bindings = MouseBindings::parse("++++");
        assert_eq!(bindings.for_button(BUTTON_MIDDLE, false), Some(SecondaryClick::Forward));
        assert_eq!(bindings.for_button(BUTTON_MIDDLE, true), Some(SecondaryClick::Forward));
    }

    #[test]
    fn the_left_button_is_never_a_secondary_click() {
        assert_eq!(MouseBindings::default().for_button(BUTTON_LEFT, false), None);
    }

    /// A wrong length or an unknown character must not silently remap the mouse.
    #[test]
    fn a_malformed_binding_keeps_the_default() {
        for spec in ["bhs", "bhsnx", "bqsn", "", "::::"] {
            let bindings = MouseBindings::parse(spec);
            assert_eq!(
                bindings.for_button(BUTTON_RIGHT, false),
                Some(SecondaryClick::Back),
                "{spec} should have been rejected"
            );
        }
    }

    #[test]
    fn every_documented_character_parses() {
        let bindings = MouseBindings::parse("-bhs:n+-b");
        assert_eq!(bindings.for_button(BUTTON_RIGHT, false), Some(SecondaryClick::Ignore));
        assert_eq!(bindings.for_button(5, false), Some(SecondaryClick::AppSwitch));
        assert_eq!(bindings.for_button(BUTTON_RIGHT, true), Some(SecondaryClick::Notifications));
    }
}
