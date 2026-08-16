//! Physical key positions, which is what a HID keyboard is made of.
//!
//! Slint reports a key as the character it produced, and a character is the
//! layout's answer rather than the key's own name — which is why UHID has been
//! out of reach here: a device told "the key that types ş" would apply the
//! Turkish layout a second time, on top of the one this machine already
//! applied. What UHID wants is the position, and the position is the one thing
//! a character cannot be turned back into.
//!
//! winit has it. `KeyEvent::physical_key` is documented as "mostly synonymous
//! with a scancode", and the Slint winit backend hands the raw winit events to
//! a `CustomApplicationHandler`. The numbers below are the USB HID Keyboard
//! usage ids (HID Usage Page 0x07), which are also SDL's scancode values —
//! scrcpy's `hid_keyboard.c` passes SDL scancodes straight through for that
//! reason, and so does `hid_keyboard.rs` here.

use slint::winit_030::winit::keyboard::KeyCode;

/// HID modifier bits, in the order the report byte has them.
pub const MOD_LEFT_CTRL: u8 = 1 << 0;
pub const MOD_LEFT_SHIFT: u8 = 1 << 1;
pub const MOD_LEFT_ALT: u8 = 1 << 2;
pub const MOD_LEFT_GUI: u8 = 1 << 3;
pub const MOD_RIGHT_CTRL: u8 = 1 << 4;
pub const MOD_RIGHT_SHIFT: u8 = 1 << 5;
pub const MOD_RIGHT_ALT: u8 = 1 << 6;
pub const MOD_RIGHT_GUI: u8 = 1 << 7;

/// The USB HID usage id for a physical key, or `None` for a key no boot-
/// protocol keyboard has.
///
/// The gaps are deliberate: the report descriptor in `hid_keyboard.rs` declares
/// usages up to 0x65, so a media key or a browser key has nowhere to go in it.
pub fn hid_usage(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::KeyA => 0x04,
        KeyCode::KeyB => 0x05,
        KeyCode::KeyC => 0x06,
        KeyCode::KeyD => 0x07,
        KeyCode::KeyE => 0x08,
        KeyCode::KeyF => 0x09,
        KeyCode::KeyG => 0x0A,
        KeyCode::KeyH => 0x0B,
        KeyCode::KeyI => 0x0C,
        KeyCode::KeyJ => 0x0D,
        KeyCode::KeyK => 0x0E,
        KeyCode::KeyL => 0x0F,
        KeyCode::KeyM => 0x10,
        KeyCode::KeyN => 0x11,
        KeyCode::KeyO => 0x12,
        KeyCode::KeyP => 0x13,
        KeyCode::KeyQ => 0x14,
        KeyCode::KeyR => 0x15,
        KeyCode::KeyS => 0x16,
        KeyCode::KeyT => 0x17,
        KeyCode::KeyU => 0x18,
        KeyCode::KeyV => 0x19,
        KeyCode::KeyW => 0x1A,
        KeyCode::KeyX => 0x1B,
        KeyCode::KeyY => 0x1C,
        KeyCode::KeyZ => 0x1D,

        KeyCode::Digit1 => 0x1E,
        KeyCode::Digit2 => 0x1F,
        KeyCode::Digit3 => 0x20,
        KeyCode::Digit4 => 0x21,
        KeyCode::Digit5 => 0x22,
        KeyCode::Digit6 => 0x23,
        KeyCode::Digit7 => 0x24,
        KeyCode::Digit8 => 0x25,
        KeyCode::Digit9 => 0x26,
        KeyCode::Digit0 => 0x27,

        KeyCode::Enter => 0x28,
        KeyCode::Escape => 0x29,
        KeyCode::Backspace => 0x2A,
        KeyCode::Tab => 0x2B,
        KeyCode::Space => 0x2C,
        KeyCode::Minus => 0x2D,
        KeyCode::Equal => 0x2E,
        KeyCode::BracketLeft => 0x2F,
        KeyCode::BracketRight => 0x30,
        KeyCode::Backslash => 0x31,
        KeyCode::Semicolon => 0x33,
        KeyCode::Quote => 0x34,
        KeyCode::Backquote => 0x35,
        KeyCode::Comma => 0x36,
        KeyCode::Period => 0x37,
        KeyCode::Slash => 0x38,
        KeyCode::CapsLock => 0x39,

        KeyCode::F1 => 0x3A,
        KeyCode::F2 => 0x3B,
        KeyCode::F3 => 0x3C,
        KeyCode::F4 => 0x3D,
        KeyCode::F5 => 0x3E,
        KeyCode::F6 => 0x3F,
        KeyCode::F7 => 0x40,
        KeyCode::F8 => 0x41,
        KeyCode::F9 => 0x42,
        KeyCode::F10 => 0x43,
        KeyCode::F11 => 0x44,
        KeyCode::F12 => 0x45,

        KeyCode::PrintScreen => 0x46,
        KeyCode::ScrollLock => 0x47,
        KeyCode::Pause => 0x48,
        KeyCode::Insert => 0x49,
        KeyCode::Home => 0x4A,
        KeyCode::PageUp => 0x4B,
        KeyCode::Delete => 0x4C,
        KeyCode::End => 0x4D,
        KeyCode::PageDown => 0x4E,
        KeyCode::ArrowRight => 0x4F,
        KeyCode::ArrowLeft => 0x50,
        KeyCode::ArrowDown => 0x51,
        KeyCode::ArrowUp => 0x52,

        KeyCode::NumLock => 0x53,
        KeyCode::NumpadDivide => 0x54,
        KeyCode::NumpadMultiply => 0x55,
        KeyCode::NumpadSubtract => 0x56,
        KeyCode::NumpadAdd => 0x57,
        KeyCode::NumpadEnter => 0x58,
        KeyCode::Numpad1 => 0x59,
        KeyCode::Numpad2 => 0x5A,
        KeyCode::Numpad3 => 0x5B,
        KeyCode::Numpad4 => 0x5C,
        KeyCode::Numpad5 => 0x5D,
        KeyCode::Numpad6 => 0x5E,
        KeyCode::Numpad7 => 0x5F,
        KeyCode::Numpad8 => 0x60,
        KeyCode::Numpad9 => 0x61,
        KeyCode::Numpad0 => 0x62,
        KeyCode::NumpadDecimal => 0x63,

        // The extra key an ISO keyboard has next to the left Shift, and the
        // menu key next to the right Ctrl.
        KeyCode::IntlBackslash => 0x64,
        KeyCode::ContextMenu => 0x65,

        // The modifiers are keys in their own right as well as bits in the
        // report's first byte: a device that never sees them pressed cannot
        // tell a held Shift from a released one.
        KeyCode::ControlLeft => 0xE0,
        KeyCode::ShiftLeft => 0xE1,
        KeyCode::AltLeft => 0xE2,
        KeyCode::SuperLeft => 0xE3,
        KeyCode::ControlRight => 0xE4,
        KeyCode::ShiftRight => 0xE5,
        KeyCode::AltRight => 0xE6,
        KeyCode::SuperRight => 0xE7,

        _ => return None,
    })
}

/// The modifier bit a key contributes to the report byte, if it is one.
pub fn modifier_bit(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::ControlLeft => MOD_LEFT_CTRL,
        KeyCode::ShiftLeft => MOD_LEFT_SHIFT,
        KeyCode::AltLeft => MOD_LEFT_ALT,
        KeyCode::SuperLeft => MOD_LEFT_GUI,
        KeyCode::ControlRight => MOD_RIGHT_CTRL,
        KeyCode::ShiftRight => MOD_RIGHT_SHIFT,
        KeyCode::AltRight => MOD_RIGHT_ALT,
        KeyCode::SuperRight => MOD_RIGHT_GUI,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spot-checked against the USB HID Usage Tables, keyboard page: these are
    /// the boundaries of each run, which is where an off-by-one would land.
    #[test]
    fn the_usage_ids_are_the_ones_the_hid_tables_give() {
        assert_eq!(hid_usage(KeyCode::KeyA), Some(0x04));
        assert_eq!(hid_usage(KeyCode::KeyZ), Some(0x1D));
        assert_eq!(hid_usage(KeyCode::Digit1), Some(0x1E), "the digits start at one");
        assert_eq!(hid_usage(KeyCode::Digit0), Some(0x27), "and zero comes last");
        assert_eq!(hid_usage(KeyCode::Enter), Some(0x28));
        assert_eq!(hid_usage(KeyCode::F1), Some(0x3A));
        assert_eq!(hid_usage(KeyCode::F12), Some(0x45));
        assert_eq!(hid_usage(KeyCode::ArrowUp), Some(0x52));
        assert_eq!(hid_usage(KeyCode::ContextMenu), Some(0x65), "the last one declared");
    }

    /// The letters and digits are two unbroken runs, so the whole of each can
    /// be checked rather than sampled.
    #[test]
    fn the_letters_and_digits_run_without_a_gap() {
        let letters = [
            KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyC, KeyCode::KeyD, KeyCode::KeyE,
            KeyCode::KeyF, KeyCode::KeyG, KeyCode::KeyH, KeyCode::KeyI, KeyCode::KeyJ,
            KeyCode::KeyK, KeyCode::KeyL, KeyCode::KeyM, KeyCode::KeyN, KeyCode::KeyO,
            KeyCode::KeyP, KeyCode::KeyQ, KeyCode::KeyR, KeyCode::KeyS, KeyCode::KeyT,
            KeyCode::KeyU, KeyCode::KeyV, KeyCode::KeyW, KeyCode::KeyX, KeyCode::KeyY,
            KeyCode::KeyZ,
        ];
        for (index, key) in letters.into_iter().enumerate() {
            assert_eq!(hid_usage(key), Some(0x04 + index as u8), "{key:?}");
        }

        let digits = [
            KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4,
            KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        for (index, key) in digits.into_iter().enumerate() {
            assert_eq!(hid_usage(key), Some(0x1E + index as u8), "{key:?}");
        }
    }

    /// Every modifier is both a key and a bit, and the two have to agree about
    /// which side of the keyboard it is on.
    #[test]
    fn the_modifiers_are_a_key_and_a_bit_at_once() {
        for (code, usage, bit) in [
            (KeyCode::ControlLeft, 0xE0, MOD_LEFT_CTRL),
            (KeyCode::ShiftLeft, 0xE1, MOD_LEFT_SHIFT),
            (KeyCode::AltLeft, 0xE2, MOD_LEFT_ALT),
            (KeyCode::SuperLeft, 0xE3, MOD_LEFT_GUI),
            (KeyCode::ControlRight, 0xE4, MOD_RIGHT_CTRL),
            (KeyCode::ShiftRight, 0xE5, MOD_RIGHT_SHIFT),
            (KeyCode::AltRight, 0xE6, MOD_RIGHT_ALT),
            (KeyCode::SuperRight, 0xE7, MOD_RIGHT_GUI),
        ] {
            assert_eq!(hid_usage(code), Some(usage), "{code:?}");
            assert_eq!(modifier_bit(code), Some(bit), "{code:?}");
            // The bit's position is the usage's distance from 0xE0.
            assert_eq!(bit, 1 << (usage - 0xE0), "{code:?}");
        }
        assert_eq!(modifier_bit(KeyCode::KeyA), None, "a letter is not a modifier");
    }

    /// A key the report descriptor has no room for must be dropped rather than
    /// sent as some other key.
    #[test]
    fn a_key_outside_the_descriptor_has_no_usage() {
        assert_eq!(hid_usage(KeyCode::AudioVolumeUp), None);
        assert_eq!(hid_usage(KeyCode::BrowserBack), None);
        assert_eq!(hid_usage(KeyCode::Power), None);
    }
}
