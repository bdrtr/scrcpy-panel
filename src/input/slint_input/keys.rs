//! Slint's characters into Android keycodes, and the shortcut table.
//!
//! Slint hands over text plus modifier flags rather than the keycode/scancode
//! pairs SDL gave upstream, so every key here is a character to be recognised —
//! including the non-printable ones, which arrive as a private unicode range.

use crate::control::control_msg::*;
use crate::control::controller::Controller;
use crate::input::keymap::{akeycode, ameta};
use crate::input::shortcuts::ShortcutAction;

/// Slint encodes non-printable keys as characters in a private unicode range.
/// See `key_codes.rs` in `i-slint-common`.
pub(super) mod key {
    pub const BACKSPACE: char = '\u{0008}';
    pub const TAB: char = '\u{0009}';
    pub const RETURN: char = '\u{000a}';
    pub const BACKTAB: char = '\u{0019}';
    pub const ESCAPE: char = '\u{001b}';
    pub const DELETE: char = '\u{007f}';
    pub const SPACE: char = ' ';

    pub const SHIFT: char = '\u{0010}';
    pub const CONTROL: char = '\u{0011}';
    pub const ALT: char = '\u{0012}';
    pub const ALT_GR: char = '\u{0013}';
    pub const CAPS_LOCK: char = '\u{0014}';
    pub const SHIFT_R: char = '\u{0015}';
    pub const CONTROL_R: char = '\u{0016}';
    pub const META: char = '\u{0017}';
    pub const META_R: char = '\u{0018}';

    pub const UP: char = '\u{F700}';
    pub const DOWN: char = '\u{F701}';
    pub const LEFT: char = '\u{F702}';
    pub const RIGHT: char = '\u{F703}';
    pub const F1: char = '\u{F704}';
    pub const F11: char = '\u{F70E}';
    pub const F12: char = '\u{F70F}';
    pub const INSERT: char = '\u{F727}';
    pub const HOME: char = '\u{F729}';
    pub const END: char = '\u{F72B}';
    pub const PAGE_UP: char = '\u{F72C}';
    pub const PAGE_DOWN: char = '\u{F72D}';
    pub const MENU: char = '\u{F735}';
}

/// Send a keycode down and straight back up.
pub(super) fn send_key(controller: &Controller, keycode: u32) {
    controller.push_msg(ControlMsg::InjectKeycode {
        action: AKEY_ACTION_DOWN,
        keycode,
        repeat: 0,
        metastate: 0,
    });
    controller.push_msg(ControlMsg::InjectKeycode {
        action: AKEY_ACTION_UP,
        keycode,
        repeat: 0,
        metastate: 0,
    });
}

/// The same shortcut table upstream had, keyed by character instead of by SDL
/// keycode.
/// Whether an action may be carried out while mirroring a camera.
///
/// The server splits its control handler in two: mirroring a camera it takes
/// the torch, the zoom and a video reset, and treats anything else as a
/// protocol error — an AssertionError on its control thread, which ends the
/// control channel for the rest of the session. So a click on a camera mirror
/// is not merely useless, and neither is Home.
///
/// The window's own actions are all allowed: they never reach the device.
pub(super) fn a_camera_takes(action: ShortcutAction) -> bool {
    use ShortcutAction as A;
    matches!(
        action,
        A::CameraTorchOn
            | A::CameraTorchOff
            | A::CameraZoomIn
            | A::CameraZoomOut
            | A::ResetVideo
            | A::ToggleFullscreen
            | A::ResizeToFit
            | A::PixelPerfect
            | A::ToggleFps
            | A::RotateCW
            | A::RotateCCW
            | A::FlipHorizontal
            | A::FlipVertical
            | A::PauseDisplay
            | A::UnpauseDisplay
            | A::Quit
            | A::None
    )
}

/// What MOD plus a key means.
///
/// `camera` decides one pair of them: scrcpy gives MOD+Up and MOD+Down to the
/// volume while mirroring a display and to the camera zoom while mirroring a
/// camera, since a camera has no volume to turn up.
pub(super) fn shortcut_for(c: char, shift: bool, camera: bool) -> ShortcutAction {
    match c.to_ascii_lowercase() {
        'h' => ShortcutAction::Home,
        'b' | key::BACKSPACE => ShortcutAction::Back,
        's' => ShortcutAction::AppSwitch,
        'p' => ShortcutAction::Power,
        'm' => ShortcutAction::Menu,
        'f' => ShortcutAction::ToggleFullscreen,
        'w' => ShortcutAction::ResizeToFit,
        'g' => ShortcutAction::PixelPerfect,
        'i' => ShortcutAction::ToggleFps,
        'q' => ShortcutAction::Quit,
        'r' if shift => ShortcutAction::ResetVideo,
        'r' => ShortcutAction::RotateDevice,
        'z' if shift => ShortcutAction::UnpauseDisplay,
        'z' => ShortcutAction::PauseDisplay,
        't' if shift => ShortcutAction::CameraTorchOff,
        't' => ShortcutAction::CameraTorchOn,
        'c' => ShortcutAction::CopyToPC,
        'x' => ShortcutAction::CutToPC,
        'v' if shift => ShortcutAction::PasteAsText,
        'v' => ShortcutAction::PasteFromPC,
        'k' => ShortcutAction::OpenKeyboardSettings,
        'n' if shift => ShortcutAction::CollapsePanels,
        'n' => ShortcutAction::ExpandNotifications,
        'o' if shift => ShortcutAction::SetDisplayPowerOn,
        'o' => ShortcutAction::SetDisplayPowerOff,
        key::UP if shift => ShortcutAction::FlipVertical,
        key::DOWN if shift => ShortcutAction::FlipVertical,
        key::UP if camera => ShortcutAction::CameraZoomIn,
        key::DOWN if camera => ShortcutAction::CameraZoomOut,
        key::UP => ShortcutAction::VolumeUp,
        key::DOWN => ShortcutAction::VolumeDown,
        key::LEFT | key::RIGHT if shift => ShortcutAction::FlipHorizontal,
        key::RIGHT => ShortcutAction::RotateCW,
        key::LEFT => ShortcutAction::RotateCCW,
        _ => ShortcutAction::None,
    }
}

/// Modifier keys arrive as key events of their own; they carry no text to
/// inject and no keycode worth forwarding.
pub(super) fn is_modifier(c: char) -> bool {
    matches!(
        c,
        key::SHIFT
            | key::CONTROL
            | key::ALT
            | key::ALT_GR
            | key::CAPS_LOCK
            | key::SHIFT_R
            | key::CONTROL_R
            | key::META
            | key::META_R
    )
}

/// Non-printable keys that always travel as Android keycodes.
pub(super) fn special_keycode(c: char) -> Option<u32> {
    Some(match c {
        key::BACKSPACE => akeycode::DEL,
        key::TAB | key::BACKTAB => akeycode::TAB,
        key::RETURN => akeycode::ENTER,
        key::ESCAPE => akeycode::ESCAPE,
        key::DELETE => akeycode::FORWARD_DEL,
        key::UP => akeycode::DPAD_UP,
        key::DOWN => akeycode::DPAD_DOWN,
        key::LEFT => akeycode::DPAD_LEFT,
        key::RIGHT => akeycode::DPAD_RIGHT,
        key::HOME => akeycode::MOVE_HOME,
        key::END => akeycode::MOVE_END,
        key::PAGE_UP => akeycode::PAGE_UP,
        key::PAGE_DOWN => akeycode::PAGE_DOWN,
        key::INSERT => akeycode::INSERT,
        key::MENU => akeycode::MENU,
        c if (key::F1..=key::F12).contains(&c) => akeycode::F1 + (c as u32 - key::F1 as u32),
        _ => return None,
    })
}

/// Printable keys that scrcpy prefers to send as keycodes rather than text:
/// the ASCII letters and space.
pub(super) fn printable_keycode(c: char) -> Option<u32> {
    if c == key::SPACE {
        return Some(akeycode::SPACE);
    }
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        Some(akeycode::A + (lower as u32 - 'a' as u32))
    } else {
        None
    }
}

/// Slint reports modifiers without a side, so only the side-agnostic Android
/// metastate bits can be set.
pub(super) fn metastate(alt: bool, control: bool, shift: bool, meta: bool) -> u32 {
    let mut state = 0;
    if shift {
        state |= ameta::SHIFT_ON;
    }
    if control {
        state |= ameta::CTRL_ON;
    }
    if alt {
        state |= ameta::ALT_ON;
    }
    if meta {
        state |= ameta::META_ON;
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// scrcpy's shortcut list, as a table: every one of them has to land on
    /// the action it names, and the ones that read Shift have to read it.
    #[test]
    fn the_shortcuts_are_the_ones_scrcpy_documents() {
        use ShortcutAction as A;
        let display = [
            ('q', false, A::Quit),
            ('f', false, A::ToggleFullscreen),
            ('h', false, A::Home),
            ('b', false, A::Back),
            (key::BACKSPACE, false, A::Back),
            ('s', false, A::AppSwitch),
            ('m', false, A::Menu),
            ('p', false, A::Power),
            ('o', false, A::SetDisplayPowerOff),
            ('o', true, A::SetDisplayPowerOn),
            ('r', false, A::RotateDevice),
            ('r', true, A::ResetVideo),
            ('n', false, A::ExpandNotifications),
            ('n', true, A::CollapsePanels),
            ('c', false, A::CopyToPC),
            ('x', false, A::CutToPC),
            ('v', false, A::PasteFromPC),
            ('v', true, A::PasteAsText),
            ('k', false, A::OpenKeyboardSettings),
            ('i', false, A::ToggleFps),
            ('g', false, A::PixelPerfect),
            ('w', false, A::ResizeToFit),
            ('z', false, A::PauseDisplay),
            ('z', true, A::UnpauseDisplay),
            ('t', false, A::CameraTorchOn),
            ('t', true, A::CameraTorchOff),
            (key::LEFT, false, A::RotateCCW),
            (key::RIGHT, false, A::RotateCW),
            (key::LEFT, true, A::FlipHorizontal),
            (key::RIGHT, true, A::FlipHorizontal),
            (key::UP, true, A::FlipVertical),
            (key::DOWN, true, A::FlipVertical),
            (key::UP, false, A::VolumeUp),
            (key::DOWN, false, A::VolumeDown),
        ];

        for (c, shift, expected) in display {
            assert_eq!(
                shortcut_for(c, shift, false),
                expected,
                "MOD+{}{:?} should be {expected:?}",
                if shift { "Shift+" } else { "" },
                c
            );
        }
    }

    /// A camera has no volume, so scrcpy gives those two keys to the zoom —
    /// and only those two change.
    #[test]
    fn a_camera_takes_the_volume_keys_for_the_zoom() {
        assert_eq!(shortcut_for(key::UP, false, true), ShortcutAction::CameraZoomIn);
        assert_eq!(shortcut_for(key::DOWN, false, true), ShortcutAction::CameraZoomOut);
        assert_eq!(shortcut_for(key::UP, true, true), ShortcutAction::FlipVertical);
        assert_eq!(shortcut_for('h', false, true), ShortcutAction::Home);
    }
}
