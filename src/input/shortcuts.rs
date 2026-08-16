
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

// The keycode-to-shortcut table that used to live here was written against SDL
// keycodes. Slint reports characters, so `input/slint_input.rs::shortcut_for`
// owns that mapping now; this file keeps the vocabulary both sides share.
