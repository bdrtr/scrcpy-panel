use sdl2::keyboard::Keycode;

/// Android keycodes (subset used by scrcpy shortcuts)
pub const AKEYCODE_HOME: u32 = 3;
pub const AKEYCODE_BACK: u32 = 4;
pub const AKEYCODE_POWER: u32 = 26;
pub const AKEYCODE_VOLUME_UP: u32 = 24;
pub const AKEYCODE_VOLUME_DOWN: u32 = 25;
pub const AKEYCODE_MENU: u32 = 82;
pub const AKEYCODE_APP_SWITCH: u32 = 187;

/// Check if a key event is a shortcut and return the action
pub enum ShortcutAction {
    Home,
    Back,
    AppSwitch,
    Power,
    VolumeUp,
    VolumeDown,
    Menu,
    ToggleFullscreen,
    ResizeToFit,
    PixelPerfect,
    ToggleFps,
    ExpandNotifications,
    ExpandSettings,
    CollapsePanels,
    RotateDevice,
    RotateCW,
    RotateCCW,
    SetDisplayPowerOff,
    SetDisplayPowerOn,
    CopyToPC,
    CutToPC,
    PasteFromPC,
    OpenKeyboardSettings,
    None,
}

/// Determine if a key press is a scrcpy shortcut
///
/// Shortcuts require Alt (LAlt or RAlt) as modifier
pub fn get_shortcut(keycode: Keycode, alt_pressed: bool, shift_pressed: bool) -> ShortcutAction {
    if !alt_pressed {
        return ShortcutAction::None;
    }

    match keycode {
        Keycode::H => ShortcutAction::Home,
        Keycode::B | Keycode::Backspace => ShortcutAction::Back,
        Keycode::S => ShortcutAction::AppSwitch,
        Keycode::P => ShortcutAction::Power,
        Keycode::M => ShortcutAction::Menu,
        Keycode::Up => ShortcutAction::VolumeUp,
        Keycode::Down => ShortcutAction::VolumeDown,
        Keycode::F => ShortcutAction::ToggleFullscreen,
        Keycode::W => ShortcutAction::ResizeToFit,
        Keycode::G => ShortcutAction::PixelPerfect,
        Keycode::I => ShortcutAction::ToggleFps,
        Keycode::N => {
            if shift_pressed {
                ShortcutAction::CollapsePanels
            } else {
                ShortcutAction::ExpandNotifications
            }
        }
        Keycode::R => ShortcutAction::RotateDevice,
        Keycode::Right => ShortcutAction::RotateCW,
        Keycode::Left => ShortcutAction::RotateCCW,
        Keycode::C => ShortcutAction::CopyToPC,
        Keycode::X => ShortcutAction::CutToPC,
        Keycode::V => ShortcutAction::PasteFromPC,
        Keycode::K => ShortcutAction::OpenKeyboardSettings,
        Keycode::O => {
            if shift_pressed {
                ShortcutAction::SetDisplayPowerOn
            } else {
                ShortcutAction::SetDisplayPowerOff
            }
        }
        _ => ShortcutAction::None,
    }
}
