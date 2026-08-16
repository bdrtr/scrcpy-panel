
/// Android keycodes (subset used by scrcpy shortcuts)
pub const AKEYCODE_HOME: u32 = 3;
pub const AKEYCODE_POWER: u32 = 26;
pub const AKEYCODE_VOLUME_UP: u32 = 24;
pub const AKEYCODE_VOLUME_DOWN: u32 = 25;
pub const AKEYCODE_MENU: u32 = 82;
pub const AKEYCODE_APP_SWITCH: u32 = 187;

/// Check if a key event is a shortcut and return the action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    CollapsePanels,
    RotateDevice,
    RotateCW,
    RotateCCW,
    SetDisplayPowerOff,
    SetDisplayPowerOn,
    CopyToPC,
    CutToPC,
    PasteFromPC,
    /// MOD+Shift+v: type the clipboard rather than hand it over, whatever
    /// --legacy-paste says.
    PasteAsText,
    OpenKeyboardSettings,
    /// MOD+q, which ends the session rather than the mirroring.
    Quit,
    /// MOD+Shift+r: ask the device to encode again from a fresh keyframe.
    ResetVideo,
    /// MOD+z and MOD+Shift+z: freeze the picture without stopping the stream.
    PauseDisplay,
    UnpauseDisplay,
    /// MOD+Shift+Left/Right and MOD+Shift+Up/Down. A vertical flip is a
    /// horizontal one turned half way round, which is how it is done here.
    FlipHorizontal,
    FlipVertical,
    /// MOD+t, MOD+Shift+t and MOD+Up/Down while mirroring a camera.
    CameraTorchOn,
    CameraTorchOff,
    CameraZoomIn,
    CameraZoomOut,
    None,
}

// The keycode-to-shortcut table that used to live here was written against SDL
// keycodes. Slint reports characters, so `input/slint_input.rs::shortcut_for`
// owns that mapping now; this file keeps the vocabulary both sides share.
