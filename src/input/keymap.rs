//! Android keycode and metastate tables (from scrcpy's keyboard_sdk.c).
//!
//! The SDL-to-Android translation that used to live here went with SDL: Slint
//! reports keys as text, so `input/slint_input.rs` maps them directly. What is
//! left is the part that was never SDL's — the AOSP constants themselves.

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

/// Android metastate flags.
///
/// Transcribed whole from Android's `KeyEvent`, because half a table is
/// harder to check against the source than all of it.
#[allow(dead_code)]
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
