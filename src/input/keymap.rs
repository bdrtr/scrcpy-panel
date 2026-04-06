//! SDL2 → Android keycode mapping (ported from scrcpy keyboard_sdk.c)

use sdl2::keyboard::{Keycode, Mod};

/// Android keycodes (matching AOSP android.view.KeyEvent)
#[allow(non_camel_case_types, dead_code)]
pub mod akeycode {
    pub const UNKNOWN: u32 = 0;
    pub const BACK: u32 = 4;
    pub const DPAD_UP: u32 = 19;
    pub const DPAD_DOWN: u32 = 20;
    pub const DPAD_LEFT: u32 = 21;
    pub const DPAD_RIGHT: u32 = 22;
    pub const VOLUME_UP: u32 = 24;
    pub const VOLUME_DOWN: u32 = 25;
    pub const POWER: u32 = 26;
    pub const HOME: u32 = 3;
    pub const MENU: u32 = 82;
    pub const APP_SWITCH: u32 = 187;
    pub const A: u32 = 29;
    pub const B: u32 = 30;
    pub const C: u32 = 31;
    pub const D: u32 = 32;
    pub const E: u32 = 33;
    pub const F: u32 = 34;
    pub const G: u32 = 35;
    pub const H: u32 = 36;
    pub const I: u32 = 37;
    pub const J: u32 = 38;
    pub const K: u32 = 39;
    pub const L: u32 = 40;
    pub const M: u32 = 41;
    pub const N: u32 = 42;
    pub const O: u32 = 43;
    pub const P: u32 = 44;
    pub const Q: u32 = 45;
    pub const R: u32 = 46;
    pub const S: u32 = 47;
    pub const T: u32 = 48;
    pub const U: u32 = 49;
    pub const V: u32 = 50;
    pub const W: u32 = 51;
    pub const X: u32 = 52;
    pub const Y: u32 = 53;
    pub const Z: u32 = 54;
    pub const K0: u32 = 7;
    pub const K1: u32 = 8;
    pub const K2: u32 = 9;
    pub const K3: u32 = 10;
    pub const K4: u32 = 11;
    pub const K5: u32 = 12;
    pub const K6: u32 = 13;
    pub const K7: u32 = 14;
    pub const K8: u32 = 15;
    pub const K9: u32 = 16;
    pub const ENTER: u32 = 66;
    pub const DEL: u32 = 67;       // Backspace
    pub const FORWARD_DEL: u32 = 112; // Delete key
    pub const ESCAPE: u32 = 111;
    pub const TAB: u32 = 61;
    pub const SPACE: u32 = 62;
    pub const MOVE_HOME: u32 = 122;
    pub const MOVE_END: u32 = 123;
    pub const PAGE_UP: u32 = 92;
    pub const PAGE_DOWN: u32 = 93;
    pub const DPAD_CENTER: u32 = 23;
    pub const COMMA: u32 = 55;
    pub const PERIOD: u32 = 56;
    pub const MINUS: u32 = 69;
    pub const EQUALS: u32 = 70;
    pub const LEFT_BRACKET: u32 = 71;
    pub const RIGHT_BRACKET: u32 = 72;
    pub const BACKSLASH: u32 = 73;
    pub const SEMICOLON: u32 = 74;
    pub const APOSTROPHE: u32 = 75;
    pub const GRAVE: u32 = 68;
    pub const SLASH: u32 = 76;
    pub const AT: u32 = 77;
    pub const PLUS: u32 = 81;
    pub const STAR: u32 = 17;
    pub const POUND: u32 = 18;
    pub const CTRL_LEFT: u32 = 113;
    pub const CTRL_RIGHT: u32 = 114;
    pub const SHIFT_LEFT: u32 = 59;
    pub const SHIFT_RIGHT: u32 = 60;
    pub const ALT_LEFT: u32 = 57;
    pub const ALT_RIGHT: u32 = 58;
    pub const META_LEFT: u32 = 117;
    pub const META_RIGHT: u32 = 118;
    pub const CAPS_LOCK: u32 = 115;
    pub const NUMPAD_0: u32 = 144;
    pub const NUMPAD_1: u32 = 145;
    pub const NUMPAD_2: u32 = 146;
    pub const NUMPAD_3: u32 = 147;
    pub const NUMPAD_4: u32 = 148;
    pub const NUMPAD_5: u32 = 149;
    pub const NUMPAD_6: u32 = 150;
    pub const NUMPAD_7: u32 = 151;
    pub const NUMPAD_8: u32 = 152;
    pub const NUMPAD_9: u32 = 153;
    pub const NUMPAD_DIVIDE: u32 = 154;
    pub const NUMPAD_MULTIPLY: u32 = 155;
    pub const NUMPAD_SUBTRACT: u32 = 156;
    pub const NUMPAD_ADD: u32 = 157;
    pub const NUMPAD_DOT: u32 = 158;
    pub const NUMPAD_ENTER: u32 = 160;
    pub const NUMPAD_EQUALS: u32 = 161;
    pub const NUMPAD_LEFT_PAREN: u32 = 162;
    pub const NUMPAD_RIGHT_PAREN: u32 = 163;
    pub const F1: u32 = 131;
    pub const F2: u32 = 132;
    pub const F3: u32 = 133;
    pub const F4: u32 = 134;
    pub const F5: u32 = 135;
    pub const F6: u32 = 136;
    pub const F7: u32 = 137;
    pub const F8: u32 = 138;
    pub const F9: u32 = 139;
    pub const F10: u32 = 140;
    pub const F11: u32 = 141;
    pub const F12: u32 = 142;
    pub const INSERT: u32 = 124;
    pub const SCROLL_LOCK: u32 = 116;
    pub const BREAK: u32 = 121;
}

/// Android metastate flags
pub mod ameta {
    pub const SHIFT_LEFT_ON: u32 = 0x40;
    pub const SHIFT_RIGHT_ON: u32 = 0x80;
    pub const SHIFT_ON: u32 = 0x1;
    pub const CTRL_LEFT_ON: u32 = 0x2000;
    pub const CTRL_RIGHT_ON: u32 = 0x4000;
    pub const CTRL_ON: u32 = 0x1000;
    pub const ALT_LEFT_ON: u32 = 0x10;
    pub const ALT_RIGHT_ON: u32 = 0x20;
    pub const ALT_ON: u32 = 0x2;
    pub const META_LEFT_ON: u32 = 0x10000;
    pub const META_RIGHT_ON: u32 = 0x20000;
    pub const META_ON: u32 = 0x10000 | 0x20000;
    pub const NUM_LOCK_ON: u32 = 0x200;
    pub const CAPS_LOCK_ON: u32 = 0x100;
}

/// Map an SDL2 Keycode to an Android keycode.
/// Returns None if the key should not be injected as a keycode
/// (e.g. regular letters/numbers which are handled by TextInput).
pub fn sdl_to_android_keycode(keycode: Keycode, keymod: Mod) -> Option<u32> {
    let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);

    // Special / navigation keys — always inject these as keycodes
    let special = match keycode {
        Keycode::Return     => Some(akeycode::ENTER),
        Keycode::KpEnter    => Some(akeycode::NUMPAD_ENTER),
        Keycode::Escape     => Some(akeycode::ESCAPE),
        Keycode::Backspace  => Some(akeycode::DEL),
        Keycode::Tab        => Some(akeycode::TAB),
        Keycode::Delete     => Some(akeycode::FORWARD_DEL),
        Keycode::Home       => Some(akeycode::MOVE_HOME),
        Keycode::End        => Some(akeycode::MOVE_END),
        Keycode::PageUp     => Some(akeycode::PAGE_UP),
        Keycode::PageDown   => Some(akeycode::PAGE_DOWN),
        Keycode::Right      => Some(akeycode::DPAD_RIGHT),
        Keycode::Left       => Some(akeycode::DPAD_LEFT),
        Keycode::Down       => Some(akeycode::DPAD_DOWN),
        Keycode::Up         => Some(akeycode::DPAD_UP),
        Keycode::Insert     => Some(akeycode::INSERT),
        Keycode::CapsLock   => Some(akeycode::CAPS_LOCK),
        Keycode::ScrollLock => Some(akeycode::SCROLL_LOCK),
        Keycode::Pause      => Some(akeycode::BREAK),
        // Modifier keys
        Keycode::LCtrl      => Some(akeycode::CTRL_LEFT),
        Keycode::RCtrl      => Some(akeycode::CTRL_RIGHT),
        Keycode::LShift     => Some(akeycode::SHIFT_LEFT),
        Keycode::RShift     => Some(akeycode::SHIFT_RIGHT),
        Keycode::LAlt       => Some(akeycode::ALT_LEFT),
        Keycode::RAlt       => Some(akeycode::ALT_RIGHT),
        Keycode::LGui       => Some(akeycode::META_LEFT),
        Keycode::RGui       => Some(akeycode::META_RIGHT),
        // Function keys
        Keycode::F1  => Some(akeycode::F1),
        Keycode::F2  => Some(akeycode::F2),
        Keycode::F3  => Some(akeycode::F3),
        Keycode::F4  => Some(akeycode::F4),
        Keycode::F5  => Some(akeycode::F5),
        Keycode::F6  => Some(akeycode::F6),
        Keycode::F7  => Some(akeycode::F7),
        Keycode::F8  => Some(akeycode::F8),
        Keycode::F9  => Some(akeycode::F9),
        Keycode::F10 => Some(akeycode::F10),
        Keycode::F11 => Some(akeycode::F11),
        Keycode::F12 => Some(akeycode::F12),
        // Numpad nav when NumLock is off
        Keycode::Kp0 => None, // handled below
        Keycode::Kp1 => None,
        Keycode::Kp2 => None,
        Keycode::Kp3 => None,
        Keycode::Kp4 => None,
        Keycode::Kp5 => None,
        Keycode::Kp6 => None,
        Keycode::Kp7 => None,
        Keycode::Kp8 => None,
        Keycode::Kp9 => None,
        Keycode::KpDivide   => Some(akeycode::NUMPAD_DIVIDE),
        Keycode::KpMultiply => Some(akeycode::NUMPAD_MULTIPLY),
        Keycode::KpMinus    => Some(akeycode::NUMPAD_SUBTRACT),
        Keycode::KpPlus     => Some(akeycode::NUMPAD_ADD),
        Keycode::KpPeriod   => Some(akeycode::NUMPAD_DOT),
        Keycode::KpEquals   => Some(akeycode::NUMPAD_EQUALS),
        _ => None,
    };

    if let Some(code) = special {
        return Some(code);
    }

    // Numpad digits
    let numpad = match keycode {
        Keycode::Kp0 => Some(akeycode::NUMPAD_0),
        Keycode::Kp1 => Some(akeycode::NUMPAD_1),
        Keycode::Kp2 => Some(akeycode::NUMPAD_2),
        Keycode::Kp3 => Some(akeycode::NUMPAD_3),
        Keycode::Kp4 => Some(akeycode::NUMPAD_4),
        Keycode::Kp5 => Some(akeycode::NUMPAD_5),
        Keycode::Kp6 => Some(akeycode::NUMPAD_6),
        Keycode::Kp7 => Some(akeycode::NUMPAD_7),
        Keycode::Kp8 => Some(akeycode::NUMPAD_8),
        Keycode::Kp9 => Some(akeycode::NUMPAD_9),
        _ => None,
    };
    if let Some(code) = numpad {
        return Some(code);
    }

    // Letters and space with Ctrl held — inject as keycode so Ctrl+C/Z etc work
    if ctrl {
        let alpha = match keycode {
            Keycode::A => Some(akeycode::A), Keycode::B => Some(akeycode::B),
            Keycode::C => Some(akeycode::C), Keycode::D => Some(akeycode::D),
            Keycode::E => Some(akeycode::E), Keycode::F => Some(akeycode::F),
            Keycode::G => Some(akeycode::G), Keycode::H => Some(akeycode::H),
            Keycode::I => Some(akeycode::I), Keycode::J => Some(akeycode::J),
            Keycode::K => Some(akeycode::K), Keycode::L => Some(akeycode::L),
            Keycode::M => Some(akeycode::M), Keycode::N => Some(akeycode::N),
            Keycode::O => Some(akeycode::O), Keycode::P => Some(akeycode::P),
            Keycode::Q => Some(akeycode::Q), Keycode::R => Some(akeycode::R),
            Keycode::S => Some(akeycode::S), Keycode::T => Some(akeycode::T),
            Keycode::U => Some(akeycode::U), Keycode::V => Some(akeycode::V),
            Keycode::W => Some(akeycode::W), Keycode::X => Some(akeycode::X),
            Keycode::Y => Some(akeycode::Y), Keycode::Z => Some(akeycode::Z),
            Keycode::Space => Some(akeycode::SPACE),
            _ => None,
        };
        if let Some(code) = alpha {
            return Some(code);
        }
    }

    // Regular letters and space without Ctrl: let TextInput handle them
    None
}

/// Convert SDL2 keyboard modifier flags to Android metastate
pub fn sdl_mod_to_metastate(keymod: Mod) -> u32 {
    let mut meta: u32 = 0;

    if keymod.contains(Mod::LSHIFTMOD) { meta |= ameta::SHIFT_LEFT_ON; }
    if keymod.contains(Mod::RSHIFTMOD) { meta |= ameta::SHIFT_RIGHT_ON; }
    if keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD) { meta |= ameta::SHIFT_ON; }

    if keymod.contains(Mod::LCTRLMOD) { meta |= ameta::CTRL_LEFT_ON; }
    if keymod.contains(Mod::RCTRLMOD) { meta |= ameta::CTRL_RIGHT_ON; }
    if keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) { meta |= ameta::CTRL_ON; }

    if keymod.contains(Mod::LALTMOD) { meta |= ameta::ALT_LEFT_ON; }
    if keymod.contains(Mod::RALTMOD) { meta |= ameta::ALT_RIGHT_ON; }
    if keymod.intersects(Mod::LALTMOD | Mod::RALTMOD) { meta |= ameta::ALT_ON; }

    if keymod.contains(Mod::LGUIMOD) { meta |= ameta::META_LEFT_ON; }
    if keymod.contains(Mod::RGUIMOD) { meta |= ameta::META_RIGHT_ON; }

    if keymod.contains(Mod::NUMMOD)  { meta |= ameta::NUM_LOCK_ON; }
    if keymod.contains(Mod::CAPSMOD) { meta |= ameta::CAPS_LOCK_ON; }

    meta
}
